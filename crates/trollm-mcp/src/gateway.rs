//! McpGateway: the single choke point for models' builtin and remote tool calls.
//! Applies the §6 per-tool policies (cache / paginate / serveFrom / shape /
//! mutation passthrough) on top of the streamable-HTTP transport.

use crate::cache::{safe_path_component, write_file_atomic, CacheStore};
use crate::events::ProxyEvent;
use crate::local;
use crate::policy::{split_policy_key, OnMiss, PolicySet, ShapePolicy, ToolClass, ToolPolicy};
use crate::preparation::{CallPreparer, PreparationContext, PreparedCall, PreparedDispatch};
use crate::schema::ToolSchema;
use crate::transport::{
    extract_tool_payload, find_items_field, required_grant_token, McpClient, RootCertificates,
    TokenGetter,
};
use crate::{Provenance, ToolCall, ToolDescriptor, ToolGateway, ToolResult};
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub type EventSink = Box<dyn FnMut(ProxyEvent) -> Result<()> + Send>;

/// Static gateway config for one MCP server; the bearer token is resolved by
/// the app (not the plan) via the getter.
pub struct ServerConfig {
    pub name: String,
    pub url: String,
    pub token_getter: Option<TokenGetter>,
    grant_bound: bool,
    allow_loopback_http: bool,
    root_certificates: RootCertificates,
    protocol_version: Option<String>,
}

impl ServerConfig {
    pub fn new(name: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            url: url.into(),
            token_getter: None,
            grant_bound: false,
            allow_loopback_http: false,
            root_certificates: RootCertificates::default(),
            protocol_version: None,
        }
    }

    /// Opt-in constructor restricted to local HTTP fixtures.
    pub fn for_local_test(name: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            url: url.into(),
            token_getter: None,
            grant_bound: false,
            allow_loopback_http: true,
            root_certificates: RootCertificates::default(),
            protocol_version: None,
        }
    }

    pub fn with_token_getter(mut self, getter: TokenGetter) -> Self {
        self.token_getter = Some(getter);
        self.grant_bound = false;
        self
    }

    /// The caller pins a local grant and endpoint. Refreshes must stay within
    /// that grant; invalidation returns None instead of selecting another account.
    pub fn with_grant_token_getter(mut self, getter: TokenGetter) -> Self {
        self.token_getter = Some(getter);
        self.grant_bound = true;
        self
    }

    /// Configure additional operator trust for this gateway's client lifetime.
    pub fn with_root_certificates(mut self, roots: RootCertificates) -> Self {
        self.root_certificates = roots;
        self
    }

    pub fn with_protocol_version(mut self, version: &str) -> Result<Self> {
        anyhow::ensure!(
            matches!(version, "2025-03-26" | "2025-06-18"),
            "unsupported MCP protocol version"
        );
        self.protocol_version = Some(version.into());
        Ok(self)
    }
}

struct ServerEntry {
    config: ServerConfig,
    client: Mutex<Option<McpClient>>,
    catalog: Mutex<Option<Arc<ToolCatalog>>>,
}

struct CatalogTool {
    descriptor: ToolDescriptor,
    schema: ToolSchema,
}

type ToolCatalog = BTreeMap<String, CatalogTool>;

impl ServerEntry {
    fn cached_catalog(&self) -> Result<Arc<ToolCatalog>> {
        self.catalog
            .lock()
            .map_err(|_| anyhow!("catalog lock poisoned"))?
            .as_ref()
            .map(Arc::clone)
            .context("tool catalog requires preparation")
    }
    fn require_credentials(&self) -> Result<()> {
        if self.config.grant_bound {
            required_grant_token(self.config.token_getter.as_ref())?;
        }
        Ok(())
    }
    /// Freeze descriptors and validators together for this gateway's captured
    /// credential context. A failed discovery never publishes a partial catalog.
    fn catalog(&self, deadline: Option<Instant>) -> Result<Arc<ToolCatalog>> {
        self.require_credentials()?;
        let mut stored = self
            .catalog
            .lock()
            .map_err(|_| anyhow!("catalog lock poisoned"))?;
        if let Some(catalog) = stored.as_ref() {
            return Ok(Arc::clone(catalog));
        }
        const MAX_PAGES: usize = 16;
        const MAX_TOOLS: usize = 1024;
        const MAX_BYTES: usize = 4 * 1024 * 1024;
        let started = Instant::now();
        let mut catalog = BTreeMap::new();
        let mut cursor: Option<String> = None;
        let mut seen_cursors = HashSet::new();
        let mut bytes = 0;
        for _ in 0..MAX_PAGES {
            let timeout = Duration::from_secs(30)
                .checked_sub(started.elapsed())
                .filter(|remaining| !remaining.is_zero())
                .context("MCP tool discovery exceeded time limit")?;
            let timeout = match deadline {
                Some(deadline) => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        bail!("run deadline exceeded during MCP tool discovery");
                    }
                    timeout.min(remaining)
                }
                None => timeout,
            };
            let page = self
                .with_client(|client| client.list_tools_page(cursor.as_deref(), Some(timeout)))
                .with_context(|| format!("MCP tool discovery failed for '{}'", self.config.name))?;
            bytes += serde_json::to_vec(&page)?.len();
            if bytes > MAX_BYTES {
                bail!("MCP tool catalog exceeds byte limit");
            }
            let tools = page
                .get("tools")
                .and_then(Value::as_array)
                .context("MCP tools/list must contain a tools array")?;
            for tool in tools {
                if started.elapsed() >= Duration::from_secs(30) {
                    bail!("MCP tool discovery exceeded time limit");
                }
                if catalog.len() >= MAX_TOOLS {
                    bail!("MCP tool catalog exceeds tool limit");
                }
                let name = tool
                    .get("name")
                    .and_then(Value::as_str)
                    .filter(|name| !name.trim().is_empty() && name.len() <= 128)
                    .context("MCP tool must have a nonempty name of at most 128 bytes")?;
                if catalog.contains_key(name) {
                    bail!("MCP tool catalog contains a duplicate name");
                }
                let input_schema = tool
                    .get("inputSchema")
                    .context("MCP tool is missing inputSchema")?;
                let schema = ToolSchema::compile(input_schema).with_context(|| {
                    format!("invalid schema for MCP tool '{}.{name}'", self.config.name)
                })?;
                catalog.insert(
                    name.to_string(),
                    CatalogTool {
                        descriptor: ToolDescriptor {
                            server: self.config.name.clone(),
                            name: name.to_string(),
                            input_schema: input_schema.clone(),
                        },
                        schema,
                    },
                );
            }
            cursor = match page.get("nextCursor") {
                None | Some(Value::Null) => None,
                Some(Value::String(cursor)) if !cursor.is_empty() && cursor.len() <= 4096 => {
                    Some(cursor.clone())
                }
                _ => bail!("MCP tools/list has an invalid nextCursor"),
            };
            match cursor.as_ref() {
                None => {
                    if started.elapsed() >= Duration::from_secs(30) {
                        bail!("MCP tool discovery exceeded time limit");
                    }
                    self.require_credentials()?;
                    let catalog = Arc::new(catalog);
                    *stored = Some(Arc::clone(&catalog));
                    return Ok(catalog);
                }
                Some(cursor) if !seen_cursors.insert(cursor.clone()) => {
                    bail!("MCP tool catalog cursor repeated")
                }
                Some(_) => {}
            }
        }
        bail!("MCP tool catalog exceeds page limit")
    }

    /// Runs `f` against a lazily created client for this server.
    fn with_client<T>(&self, f: impl FnOnce(&mut McpClient) -> Result<T>) -> Result<T> {
        let mut guard = self
            .client
            .lock()
            .map_err(|_| anyhow!("client lock poisoned"))?;
        if guard.is_none() {
            let client = if self.config.allow_loopback_http {
                McpClient::for_local_test_with_root_certificates(
                    self.config.url.clone(),
                    self.config.token_getter.clone(),
                    self.config.root_certificates.clone(),
                )?
            } else {
                McpClient::with_root_certificates(
                    self.config.url.clone(),
                    self.config.token_getter.clone(),
                    self.config.root_certificates.clone(),
                )?
            };
            let client = if let Some(version) = &self.config.protocol_version {
                client.with_protocol_version(version)?
            } else {
                client
            };
            *guard = Some(if self.config.grant_bound {
                client.require_grant_credentials()
            } else {
                client
            });
        }
        f(guard.as_mut().expect("client just initialized"))
    }
}

#[derive(Clone, Copy, Default)]
struct DispatchTimings {
    transport: Option<Duration>,
    cache_read: Option<Duration>,
    prepare_result: Option<Duration>,
    finish_result: Option<Duration>,
}

pub struct McpGateway {
    required_preparations: HashSet<String>,
    preparer: Option<Arc<dyn CallPreparer>>,
    pending_preparation: Option<PreparedDispatch>,
    used_preparation_contexts: HashSet<String>,
    active_preparation: Option<PreparedDispatch>,
    awaiting_retained_result: Option<PreparedDispatch>,
    preparation_fenced: bool,
    send_gate: Option<Arc<dyn Fn() -> Result<()> + Send + Sync>>,
    call_context: Option<crate::CallContext>,
    timings: std::cell::Cell<DispatchTimings>,
    dispatch_started: Option<std::time::Instant>,
    dispatch_id: u64,
    dispatch_args_hash: String,
    deadline: Option<std::time::Instant>,
    servers: HashMap<String, ServerEntry>,
    local_tools: HashMap<String, CatalogTool>,
    policies: PolicySet,
    task_artifact_owners: HashMap<String, String>,
    result_validators: HashMap<String, ToolSchema>,
    argument_constraints: HashMap<String, ToolSchema>,
    argument_constraint_schemas: std::collections::BTreeMap<String, Value>,
    cache: CacheStore,
    artifact_dir: PathBuf,
    sink: Option<Arc<Mutex<EventSink>>>,
    audit_failed: AtomicBool,
}

impl McpGateway {
    pub fn new(
        mut servers: Vec<ServerConfig>,
        policies: PolicySet,
        cache_root: PathBuf,
        artifact_dir: PathBuf,
        sink: Option<EventSink>,
    ) -> Result<Self> {
        policies.validate()?;
        for (key, policy) in &policies.policies {
            if let Some((server, tool)) = split_policy_key(key) {
                if server == local::SERVER {
                    if !local::supports(tool) {
                        bail!("unknown builtin local tool in policy '{key}'");
                    }
                    if policy.class != ToolClass::Read
                        || policy.cache.is_some()
                        || policy.paginate.is_some()
                        || policy.serve_from.is_some()
                        || policy.on_miss.is_some()
                        || policy.passthrough
                    {
                        bail!("builtin local tool policy '{key}' only supports read, resultArtifact, resultValidation, shape, timeoutS and default retry");
                    }
                }
            }
        }
        let mut names = HashSet::new();
        for server in &servers {
            if server.name == "flow" {
                bail!("MCP server name 'flow' is reserved for runtime control tools");
            }
            if server.name == local::SERVER {
                bail!("MCP server name 'local' is reserved for builtin tools");
            }
            if server.name.trim().is_empty() || !names.insert(server.name.clone()) {
                bail!("MCP server names must be nonempty and unique");
            }
            McpClient::validate_url(&server.url, server.allow_loopback_http)?;
        }
        // Result schemas belong to this immutable gateway policy generation,
        // not to an individual call or a model-facing shaped copy.
        let result_validators = policies
            .policies
            .iter()
            .filter_map(|(key, policy)| {
                policy.result_validation.as_ref().map(|validation| {
                    ToolSchema::compile_result(&validation.schema)
                        .map(|compiled| (key.clone(), compiled))
                })
            })
            .collect::<Result<HashMap<_, _>>>()?;
        // Freeze legacy bearer callbacks. Grant-bound callbacks may refresh
        // tokens, but the caller must keep their grant and endpoint fixed.
        for server in &mut servers {
            if server.grant_bound {
                continue;
            }
            let token = server.token_getter.as_ref().and_then(|getter| getter());
            server.token_getter =
                token.map(|token| Arc::new(move || Some(token.clone())) as TokenGetter);
        }
        // Each gateway owns its generation and never reads another run's cache.
        // This deliberately gives up persistent cross-run reuse until a shared
        // generation publication protocol exists.
        static NEXT_NAMESPACE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let nonce = NEXT_NAMESPACE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let cache_root = cache_root.join(format!("gateway-{}-{nanos}-{nonce}", std::process::id()));
        let servers = servers
            .into_iter()
            .map(|config| {
                (
                    config.name.clone(),
                    ServerEntry {
                        config,
                        client: Mutex::new(None),
                        catalog: Mutex::new(None),
                    },
                )
            })
            .collect();
        Ok(Self {
            required_preparations: HashSet::new(),
            preparer: None,
            pending_preparation: None,
            used_preparation_contexts: HashSet::new(),
            active_preparation: None,
            awaiting_retained_result: None,
            preparation_fenced: false,
            send_gate: None,
            call_context: None,
            timings: std::cell::Cell::new(DispatchTimings::default()),
            dispatch_started: None,
            dispatch_id: 0,
            dispatch_args_hash: String::new(),
            deadline: None,
            servers,
            local_tools: local::descriptors()
                .into_iter()
                .map(|descriptor| {
                    Ok((
                        descriptor.name.clone(),
                        CatalogTool {
                            schema: ToolSchema::compile(&descriptor.input_schema)?,
                            descriptor,
                        },
                    ))
                })
                .collect::<Result<HashMap<_, _>>>()?,
            policies,
            task_artifact_owners: HashMap::new(),
            result_validators,
            argument_constraints: HashMap::new(),
            argument_constraint_schemas: Default::default(),
            cache: CacheStore::new(cache_root),
            artifact_dir,
            sink: sink.map(|s| Arc::new(Mutex::new(s))),
            audit_failed: AtomicBool::new(false),
        })
    }

    /// Closed native installation input. A selected mutation cannot use a
    /// direct gateway call to bypass preparation or reuse another call's permit.
    pub fn with_preparer(
        mut self,
        tools: Vec<String>,
        preparer: Arc<dyn CallPreparer>,
    ) -> Result<Self> {
        anyhow::ensure!(
            !tools.is_empty() && tools.len() <= 16,
            "invalid preparation tool count"
        );
        for tool in tools {
            let (server, name) = split_policy_key(&tool).context("invalid preparation tool")?;
            anyhow::ensure!(
                server != local::SERVER && self.servers.contains_key(server),
                "preparation server unavailable"
            );
            let config = &self.servers[server].config;
            anyhow::ensure!(
                config.grant_bound && config.protocol_version.is_some(),
                "prepared tools require an explicit protocol and native-bound credentials"
            );
            let policy = self
                .policies
                .get(server, name)
                .context("preparation requires explicit mutation policy")?;
            anyhow::ensure!(
                policy.class == ToolClass::Mutation && policy.passthrough,
                "prepared tools require mutation passthrough"
            );
            anyhow::ensure!(
                self.required_preparations.insert(tool),
                "duplicate preparation tool"
            );
        }
        self.preparer = Some(preparer);
        Ok(self)
    }

    /// Nonblocking native cancellation/currentness gate, evaluated after the
    /// client lock and credential callback, immediately before HTTP send.
    pub fn with_send_gate(mut self, gate: Arc<dyn Fn() -> Result<()> + Send + Sync>) -> Self {
        self.send_gate = Some(gate);
        self
    }

    fn prepared_for(&self, call: &ToolCall) -> Result<Option<&PreparedDispatch>> {
        if !self
            .required_preparations
            .contains(&crate::policy::policy_key(&call.server, &call.tool))
        {
            return Ok(None);
        }
        let p = self
            .active_preparation
            .as_ref()
            .or(self.pending_preparation.as_ref())
            .context("selected tool requires native preparation")?;
        anyhow::ensure!(
            self.call_context.as_ref() == Some(&p.context.call_context),
            "prepared call context mismatch"
        );
        anyhow::ensure!(
            p.prepared.call.server == call.server
                && p.prepared.call.tool == call.tool
                && p.prepared.call.args == call.args,
            "prepared final arguments mismatch"
        );
        Ok(Some(p))
    }

    fn require_unused_selected_result(
        &self,
        call: &ToolCall,
        context: &PreparationContext,
    ) -> Result<()> {
        if let Some(name) = self
            .policies
            .get(&call.server, &call.tool)
            .and_then(|policy| policy.result_artifact.as_ref())
        {
            let path = self
                .artifact_dir
                .join(crate::preparation::selected_result_artifact_name(
                    name, context,
                )?);
            let metadata_path = PathBuf::from(format!("{}.meta.json", path.display()));
            for candidate in [&path, &metadata_path] {
                match std::fs::symlink_metadata(candidate) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    _ => bail!("selected result destination already exists or is unavailable"),
                }
            }
        }
        Ok(())
    }

    fn ensure_catalog_for(&self, server: &str) -> Result<()> {
        if server != local::SERVER {
            self.server(server)?.catalog(self.deadline)?;
        }
        Ok(())
    }

    /// Additional immutable run-local restrictions, checked alongside catalog schemas.
    pub fn with_argument_constraints(
        mut self,
        constraints: std::collections::BTreeMap<String, Value>,
    ) -> Result<Self> {
        if constraints.len() > 16 {
            bail!("at most 16 tool argument constraints are supported");
        }
        let argument_constraint_schemas = constraints.clone();
        let compiled = constraints
            .into_iter()
            .map(|(key, schema)| {
                let (server, tool) = key
                    .split_once('.')
                    .context("argument constraint key must be server.tool")?;
                if !is_local_tool(server, tool)
                    && (!self.servers.contains_key(server) || tool.is_empty())
                {
                    bail!("argument constraint must name a declared server and exact tool");
                }
                Ok((key, ToolSchema::compile(&schema)?))
            })
            .collect::<Result<HashMap<_, _>>>()?;
        self.argument_constraints = compiled;
        self.argument_constraint_schemas = argument_constraint_schemas;
        Ok(self)
    }

    pub fn set_event_sink(&mut self, sink: EventSink) {
        self.sink = Some(Arc::new(Mutex::new(sink)));
    }

    pub fn cache(&self) -> &CacheStore {
        &self.cache
    }

    fn require_audit(&self) -> Result<()> {
        if self.audit_failed.load(Ordering::SeqCst) {
            bail!("proxy audit unavailable; further MCP dispatch prohibited");
        }
        Ok(())
    }

    fn emit(&self, event: ProxyEvent) -> Result<()> {
        self.require_audit()?;
        if let Some(sink) = &self.sink {
            let result = sink
                .lock()
                .map_err(|_| anyhow!("proxy audit sink lock poisoned"))
                .and_then(|mut sink| sink(event));
            if result.is_err() {
                self.audit_failed.store(true, Ordering::SeqCst);
            }
            result.context("proxy audit write failed; further MCP dispatch prohibited")?;
        }
        Ok(())
    }

    fn server(&self, name: &str) -> Result<&ServerEntry> {
        self.servers
            .get(name)
            .ok_or_else(|| anyhow!("unknown MCP server '{name}'"))
    }

    fn require_tool_credentials(&self, server: &str, tool: &str) -> Result<()> {
        if is_local_tool(server, tool) {
            return Ok(());
        }
        self.server(server)?.require_credentials()
    }

    fn bounded_timeout(&self, timeout: Duration) -> Result<Duration> {
        match self.deadline {
            Some(deadline) => {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() {
                    bail!("run deadline exceeded before MCP dispatch");
                }
                Ok(timeout.min(remaining))
            }
            None => Ok(timeout),
        }
    }

    fn call_live(
        &self,
        server: &str,
        tool: &str,
        args: &Value,
        timeout_s: Option<u64>,
    ) -> Result<Value> {
        self.require_audit()?;
        self.validate_arguments(server, tool, args)?;
        let timeout = Some(self.bounded_timeout(Duration::from_secs(timeout_s.unwrap_or(30)))?);
        let result = self.measured("transport", || {
            self.server(server)?.with_client(|client| {
                let call = ToolCall {
                    server: server.into(),
                    tool: tool.into(),
                    args: args.clone(),
                };
                let check = || {
                    if let Some(gate) = &self.send_gate {
                        gate()?;
                    }
                    if let Some(p) = self.prepared_for(&call)? {
                        self.require_unused_selected_result(&call, &p.context)?;
                        anyhow::ensure!(self.send_gate.is_some(), "native send gate unavailable");
                        self.preparer
                            .as_ref()
                            .context("preparation provider unavailable")?
                            .check_before_send(&call, &p.context, &p.prepared.evidence)?;
                    }
                    if let Some(gate) = &self.send_gate {
                        gate()?;
                    }
                    self.bounded_timeout(Duration::from_secs(30))?;
                    Ok(())
                };
                if self.prepared_for(&call)?.is_some() {
                    let acquisition = || {
                        let p = self
                            .prepared_for(&call)?
                            .context("selected preparation lost before acquisition")?;
                        self.preparer
                            .as_ref()
                            .context("preparation provider unavailable")?
                            .acquire_before_send(&call, &p.context, &p.prepared.evidence)
                    };
                    client.call_prepared_tool_acquired(tool, args, timeout, &acquisition, &check)
                } else {
                    client.call_tool_checked(tool, args, timeout, Some(&check))
                }
            })
        })?;
        let payload = extract_tool_payload(&result);
        self.response_size(
            server,
            tool,
            "raw_payload",
            "live",
            Some(crate::json_size::compact_bytes(&payload)?),
            None,
        )?;
        Ok(payload)
    }

    fn validate_argument_constraints(&self, server: &str, tool: &str, args: &Value) -> Result<()> {
        if let Some(constraint) = self.argument_constraints.get(&format!("{server}.{tool}")) {
            if server == local::SERVER && (tool == local::SELECT_TOOL || local::record_tool(tool)) {
                constraint.validate_with_byte_limit(args, local::SELECT_ARGUMENT_BYTES)?;
            } else {
                constraint.validate(args)?;
            }
        }
        Ok(())
    }

    fn validate_arguments(&self, server: &str, tool: &str, args: &Value) -> Result<()> {
        if !args.is_object() {
            bail!("MCP tool arguments must be an object");
        }
        self.validate_argument_constraints(server, tool, args)?;
        if server == local::SERVER {
            if !local::supports(tool) {
                bail!("unknown builtin local tool");
            }
            let entry = self
                .local_tools
                .get(tool)
                .context("unknown builtin local tool")?;
            if tool == local::SELECT_TOOL || local::record_tool(tool) {
                entry
                    .schema
                    .validate_with_byte_limit(args, local::SELECT_ARGUMENT_BYTES)?;
                if local::record_tool(tool) {
                    local::validate_record_arguments(tool, args)?;
                } else {
                    local::validate_select_arguments(args)?;
                }
            } else {
                entry.schema.validate(args)?;
            }
            if tool == local::PREPARE_TOOL {
                local::validate_prepare_arguments(args)?;
            }
            if tool == local::COLLECTION_TOOL {
                local::validate_collection_arguments(args)?;
            }
            if tool == local::EXTRACT_TOOL {
                local::validate_extract_arguments(args)?;
            }
            return Ok(());
        }
        let catalog = self.server(server)?.cached_catalog()?;
        let entry = catalog
            .get(tool)
            .context("unknown tool in MCP server catalog")?;
        entry.schema.validate(args)
    }
}

impl ToolGateway for McpGateway {
    fn after_retained_intent(
        &mut self,
        call: &ToolCall,
        context: &PreparationContext,
        intent_seq: Option<u64>,
    ) -> Result<()> {
        if !self
            .required_preparations
            .contains(&crate::policy::policy_key(&call.server, &call.tool))
        {
            return Ok(());
        }
        anyhow::ensure!(!self.preparation_fenced, "selected preparation fenced");
        self.preparation_fenced = true;
        let pending = self
            .pending_preparation
            .as_ref()
            .context("selected preparation missing before intent")?;
        anyhow::ensure!(
            pending.context.raw()?.sha256 == context.raw()?.sha256
                && pending.prepared.call.server == call.server
                && pending.prepared.call.tool == call.tool
                && pending.prepared.call.args == call.args,
            "selected native intent association mismatch"
        );
        let seq = intent_seq
            .filter(|seq| *seq > 0)
            .context("selected native intent unavailable")?;
        self.preparer
            .as_ref()
            .context("selected preparer unavailable")?
            .after_retained_intent(call, context, &pending.prepared.evidence, seq)?;
        self.pending_preparation
            .as_mut()
            .context("selected preparation disappeared")?
            .intent_seq = Some(seq);
        self.preparation_fenced = false;
        Ok(())
    }
    fn after_retained_result(
        &mut self,
        call: &ToolCall,
        context: &PreparationContext,
        result: &ToolResult,
        intent_seq: Option<u64>,
        result_seq: u64,
    ) -> Result<()> {
        if !self
            .required_preparations
            .contains(&crate::policy::policy_key(&call.server, &call.tool))
        {
            return Ok(());
        }
        // Remain fenced on any mismatch or failed acknowledgement. Reporting
        // recovery may not create another dispatch permit.
        self.preparation_fenced = true;
        let pending = self
            .awaiting_retained_result
            .as_ref()
            .context("selected retained result is not pending")?;
        anyhow::ensure!(
            pending.context.raw()?.sha256 == context.raw()?.sha256
                && pending.prepared.call.server == call.server
                && pending.prepared.call.tool == call.tool
                && pending.prepared.call.args == call.args,
            "retained result association mismatch"
        );
        anyhow::ensure!(
            intent_seq == pending.intent_seq && intent_seq.is_some_and(|seq| seq < result_seq),
            "selected mutation requires preceding native intent"
        );
        self.preparer
            .as_ref()
            .context("preparer unavailable")?
            .after_retained_result(
                call,
                context,
                &pending.prepared.evidence,
                result,
                intent_seq,
                result_seq,
            )?;
        self.awaiting_retained_result = None;
        self.preparation_fenced = false;
        Ok(())
    }
    fn prepare_call(
        &mut self,
        call: &ToolCall,
        context: &PreparationContext,
    ) -> Result<Option<PreparedCall>> {
        if !self
            .required_preparations
            .contains(&crate::policy::policy_key(&call.server, &call.tool))
        {
            self.validate_argument_constraints(&call.server, &call.tool, &call.args)?;
        }
        if !self
            .required_preparations
            .contains(&crate::policy::policy_key(&call.server, &call.tool))
        {
            self.ensure_catalog_for(&call.server)?;
            return Ok(None);
        }
        anyhow::ensure!(
            !self.preparation_fenced
                && self.awaiting_retained_result.is_none()
                && self.pending_preparation.is_none()
                && self.active_preparation.is_none(),
            "preparation already pending"
        );
        anyhow::ensure!(
            self.call_context.as_ref() == Some(&context.call_context),
            "native context not selected"
        );
        anyhow::ensure!(
            self.used_preparation_contexts.len() < 4096,
            "preparation context capacity exceeded"
        );
        anyhow::ensure!(
            self.used_preparation_contexts.insert(context.raw()?.sha256),
            "native preparation context already used"
        );
        self.preparation_fenced = true;
        self.require_unused_selected_result(call, context)?;
        self.ensure_catalog_for(&call.server)?;
        let prepared = self
            .preparer
            .as_ref()
            .context("preparation provider unavailable")?
            .prepare(call, context, self.deadline)?;
        prepared.validate(call, context)?;
        self.bounded_timeout(Duration::from_secs(30))?;
        if let Some(gate) = &self.send_gate {
            gate()?;
        }
        self.pending_preparation = Some(PreparedDispatch {
            intent_seq: None,
            context: context.clone(),
            prepared: prepared.clone(),
        });
        self.preparation_fenced = false;
        Ok(Some(prepared))
    }
    fn set_call_context(&mut self, context: Option<crate::CallContext>) {
        if context != self.call_context {
            self.preparation_fenced |= self.pending_preparation.is_some()
                || self.active_preparation.is_some()
                || self.awaiting_retained_result.is_some();
            self.pending_preparation = None;
        }
        self.call_context = context;
    }

    fn set_deadline(&mut self, deadline: Option<std::time::Instant>) {
        self.deadline = deadline;
    }
    fn prepared_argument_names(&self, server: &str, tool: &str) -> Vec<String> {
        if !self
            .required_preparations
            .contains(&crate::policy::policy_key(server, tool))
        {
            return Vec::new();
        }
        self.preparer
            .as_ref()
            .map(|p| p.prepared_argument_names(server, tool))
            .unwrap_or_default()
    }
    fn argument_constraints(&self, server: &str, tool: &str) -> Option<Value> {
        self.argument_constraint_schemas
            .get(&format!("{server}.{tool}"))
            .cloned()
    }

    fn tool_class(&self, server: &str, tool: &str) -> ToolClass {
        self.policies
            .get(server, tool)
            .map_or(ToolClass::Read, |policy| policy.class)
    }

    fn list_tools(&self) -> Result<Vec<ToolDescriptor>> {
        self.require_audit()?;
        let mut out: Vec<_> = self
            .local_tools
            .values()
            .map(|entry| entry.descriptor.clone())
            .collect();
        for entry in self.servers.values() {
            out.extend(
                entry
                    .catalog(self.deadline)?
                    .values()
                    .map(|tool| tool.descriptor.clone()),
            );
        }
        out.sort_by(|a, b| (&a.server, &a.name).cmp(&(&b.server, &b.name)));
        Ok(out)
    }

    fn available_tools(&self, grants: &[String]) -> Vec<ToolDescriptor> {
        let granted = |descriptor: &ToolDescriptor| {
            let key = format!("{}.{}", descriptor.server, descriptor.name);
            grants.contains(&key)
        };
        let mut out: Vec<_> = self
            .local_tools
            .values()
            .filter(|entry| granted(&entry.descriptor))
            .map(|entry| entry.descriptor.clone())
            .collect();
        for entry in self.servers.values() {
            // Discovery may hold this lock during network I/O. Guidance must
            // neither wait for it nor initialize a catalog or credentials.
            let Ok(stored) = entry.catalog.try_lock() else {
                continue;
            };
            if let Some(catalog) = stored.as_ref() {
                out.extend(
                    catalog
                        .values()
                        .filter(|entry| granted(&entry.descriptor))
                        .map(|entry| entry.descriptor.clone()),
                );
            }
        }
        out.sort_by(|a, b| (&a.server, &a.name).cmp(&(&b.server, &b.name)));
        out
    }

    fn validate_call(&self, call: &ToolCall) -> Result<()> {
        self.require_audit()?;
        self.prepared_for(call)?;
        self.validate_argument_constraints(&call.server, &call.tool, &call.args)?;
        // Pagination exposes a positive page selector to the caller; actual
        // server cursors are supplied by responses and checked before every I/O.
        if let Some(policy) = self.policies.get(&call.server, &call.tool) {
            if policy.class != ToolClass::Mutation
                && policy.serve_from.is_none()
                && policy.cache.is_some()
            {
                if let Some(paginate) = &policy.paginate {
                    let mut args = call.args.clone();
                    let fields = args
                        .as_object_mut()
                        .context("paginated arguments must be an object")?;
                    if let Some(page) = fields.remove(&paginate.param) {
                        if page.as_u64().is_none_or(|page| page == 0) {
                            bail!("pagination page selector must be a positive integer");
                        }
                    }
                    return self.validate_arguments(&call.server, &call.tool, &args);
                }
            }
        }
        self.validate_arguments(&call.server, &call.tool, &call.args)
    }

    fn begin_result_artifact_attempt(
        &mut self,
        server: &str,
        tool: &str,
        name: &str,
    ) -> Result<()> {
        self.policies.task_result_policy(server, tool, name)?;
        let owner = crate::policy::policy_key(server, tool);
        anyhow::ensure!(
            self.task_artifact_owners
                .get(name)
                .is_none_or(|previous| previous == &owner),
            "task resultArtifact collides with another producer tool"
        );
        self.task_artifact_owners.insert(name.to_owned(), owner);
        let path = self.artifact_dir.join(format!("{name}.json.meta.json"));
        write_file_atomic(&path, &serde_json::to_vec(&json!({"complete": false}))?)?;
        Ok(())
    }

    fn call(&mut self, call: &ToolCall) -> Result<ToolResult> {
        self.call_with_result_artifact(call, None)
    }

    fn call_with_result_artifact(
        &mut self,
        call: &ToolCall,
        name: Option<&str>,
    ) -> Result<ToolResult> {
        if self
            .required_preparations
            .contains(&crate::policy::policy_key(&call.server, &call.tool))
        {
            anyhow::ensure!(
                !self.preparation_fenced
                    && self.awaiting_retained_result.is_none()
                    && self.active_preparation.is_none(),
                "prepared dispatch active, awaiting acknowledgement or fenced"
            );
            self.active_preparation = Some(
                self.pending_preparation
                    .take()
                    .context("selected tool requires native preparation")?,
            );
        }
        let result = (|| {
            if let Some(prepared) = self.prepared_for(call)? {
                anyhow::ensure!(
                    prepared.intent_seq.is_some(),
                    "selected effect requires verified native intent before dispatch"
                );
            }
            self.validate_argument_constraints(&call.server, &call.tool, &call.args)?;
            self.ensure_catalog_for(&call.server)?;
            if let Some(name) = name {
                self.begin_result_artifact_attempt(&call.server, &call.tool, name)?;
            }
            // Invalid calls never cross the dispatch/audit boundary.
            self.validate_call(call)?;
            self.timings.set(DispatchTimings::default());
            self.dispatch_started = Some(std::time::Instant::now());
            self.dispatch_id += 1;
            self.dispatch_args_hash = CacheStore::hash_args(&call.args, None, None);
            let result = self.dispatch(call, name);
            // Preserve the original audit I/O failure; the sink is already closed.
            if self.audit_failed.load(Ordering::SeqCst) {
                return result;
            }
            match &result {
                Ok(result) => self.response_size(
                    &call.server,
                    &call.tool,
                    "gateway_result",
                    provenance_source(result.provenance),
                    Some(crate::json_size::compact_bytes(&result.content_for_model)?),
                    None,
                )?,
                Err(error) => {
                    if let Some(received) =
                        error.downcast_ref::<crate::transport::ReceivedResponseError>()
                    {
                        self.response_size(
                            &call.server,
                            &call.tool,
                            "raw_error_payload",
                            &format!("live:{}", received.origin),
                            Some(received.compact_json_utf8_bytes),
                            None,
                        )?;
                    }
                    self.response_size(
                        &call.server,
                        &call.tool,
                        "failed_result",
                        "unknown",
                        None,
                        None,
                    )?;
                }
            }
            result
        })();
        // Includes initialization/catalog failures before dispatch as well as
        // a received failure after a possible selected write. Retention never
        // changes the original refusal into an acknowledgement or a retry.
        let result = match result {
            Err(error) => {
                if let Some(received) =
                    error.downcast_ref::<crate::transport::HttpResponseFailure>()
                {
                    if self.retain_http_response(call, received).is_err() {
                        Err(error.context("MCP_HTTP_DIAGNOSTIC_UNAVAILABLE"))
                    } else {
                        Err(error)
                    }
                } else {
                    Err(error)
                }
            }
            ok => ok,
        };
        if let Some(active) = self.active_preparation.take() {
            if result.is_ok() {
                self.awaiting_retained_result = Some(active);
            } else {
                self.preparation_fenced = true;
            }
        }
        result
    }
}

impl McpGateway {
    fn measured<T>(&self, phase: &str, f: impl FnOnce() -> Result<T>) -> Result<T> {
        let started = std::time::Instant::now();
        let result = f();
        let mut metrics = self.timings.get();
        let target = match phase {
            "transport" => &mut metrics.transport,
            "cache" => &mut metrics.cache_read,
            "admission" => &mut metrics.prepare_result,
            "finish" => &mut metrics.finish_result,
            _ => unreachable!("internal timing phase"),
        };
        *target = Some(target.unwrap_or_default().saturating_add(started.elapsed()));
        self.timings.set(metrics);
        result
    }

    fn retain_http_response(
        &self,
        call: &ToolCall,
        response: &crate::transport::HttpResponseFailure,
    ) -> Result<()> {
        use std::io::Write;
        let args_hash = CacheStore::hash_args(&call.args, None, None);
        let original = serde_json::to_vec(&json!({
            "response": response.private,
            "server": call.server, "tool": call.tool, "argsHash": args_hash,
            "callContext": self.call_context,
            "scope": "private-http-originals-not-effect-settlement"
        }))?;
        anyhow::ensure!(original.len() <= 512 * 1024, "MCP_HTTP_DIAGNOSTIC_LIMIT");
        let digest = format!("{:x}", Sha256::digest(&original));
        std::fs::create_dir_all(&self.artifact_dir)?;
        let path = self
            .artifact_dir
            .join(format!("mcp-http-failure-{digest}.json"));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(mut file) => {
                file.write_all(&original)?;
                file.sync_all()?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                anyhow::ensure!(
                    std::fs::symlink_metadata(&path)?.file_type().is_file()
                        && std::fs::metadata(&path)?.len() == original.len() as u64
                        && std::fs::read(&path)? == original,
                    "MCP_HTTP_DIAGNOSTIC_CONFLICT"
                );
            }
            Err(error) => return Err(error.into()),
        }
        std::fs::File::open(&self.artifact_dir)?.sync_all()?;
        self.emit(ProxyEvent::HttpResponseRefused {
            server: call.server.clone(),
            tool: call.tool.clone(),
            args_hash,
            status: response.status,
            cause: response.cause.to_owned(),
            diagnostic_path: path,
            diagnostic_sha256: digest,
            call_context: self.call_context.clone(),
        })
    }

    fn response_size(
        &self,
        server: &str,
        tool: &str,
        stage: &str,
        source: &str,
        bytes: Option<u64>,
        page: Option<u64>,
    ) -> Result<()> {
        self.emit(ProxyEvent::ResponseSize {
            server: server.to_string(),
            tool: tool.to_string(),
            args_hash: self.dispatch_args_hash.clone(),
            dispatch_id: self.dispatch_id,
            stage: stage.to_string(),
            source: source.to_string(),
            compact_json_utf8_bytes: bytes,
            page,
            call_context: self.call_context.clone(),
            timings: self.dispatch_started.map(|start| {
                let timing = self.timings.get();
                let ms =
                    |d: Option<Duration>| d.map(|v| v.as_millis().min(u64::MAX as u128) as u64);
                json!({
                    "elapsedMs": ms(Some(start.elapsed())),
                    "transportMs": ms(timing.transport),
                    "cacheReadMs": ms(timing.cache_read),
                    "resultAdmissionMs": ms(timing.prepare_result),
                    "resultFinishMs": ms(timing.finish_result),
                })
            }),
        })
    }

    fn dispatch(&mut self, call: &ToolCall, name: Option<&str>) -> Result<ToolResult> {
        let policy = match name {
            Some(name) => Some(
                self.policies
                    .task_result_policy(&call.server, &call.tool, name)?,
            ),
            None => self.policies.get(&call.server, &call.tool).cloned(),
        };
        match &policy {
            Some(p) if p.class == ToolClass::Mutation => self.call_mutation(call, p),
            Some(p) if p.serve_from.is_some() => self.call_serve_from(call, p),
            Some(p) if p.paginate.is_some() && p.cache.is_some() => self.call_paginated(call, p),
            Some(p) if p.cache.is_some() => self.call_cached_read(call, p),
            _ => {
                // Read calls without a strategy share validation and publication.
                let args_hash = CacheStore::hash_args(&call.args, None, None);
                if is_local_tool(&call.server, &call.tool) {
                    self.emit(ProxyEvent::LocalCall {
                        server: call.server.clone(),
                        tool: call.tool.clone(),
                        args_hash,
                    })?;
                    let payload = local::invoke(&call.tool, &call.args)?;
                    return self.finish_result(call, policy.as_ref(), payload, Provenance::Local);
                }
                let payload = self.call_live(
                    &call.server,
                    &call.tool,
                    &call.args,
                    policy.as_ref().and_then(|p| p.timeout_s),
                )?;
                self.emit(ProxyEvent::LiveCall {
                    server: call.server.clone(),
                    tool: call.tool.clone(),
                    args_hash,
                })?;
                self.finish_result(call, policy.as_ref(), payload, Provenance::Live)
            }
        }
    }
}

impl McpGateway {
    /// Mutation class: live passthrough only — never cached or retried; the
    /// intent event is emitted BEFORE execution (core journals it for
    /// crash-safe resume); ambiguous failures are never retried.
    fn call_mutation(&mut self, call: &ToolCall, policy: &ToolPolicy) -> Result<ToolResult> {
        let args_hash = CacheStore::hash_args(&call.args, None, None);
        self.emit(ProxyEvent::MutationPassthrough {
            server: call.server.clone(),
            tool: call.tool.clone(),
            args_hash: args_hash.clone(),
        })?;
        // A lost response may follow an applied business effect. Never retry mutations.
        let payload = self.call_live(&call.server, &call.tool, &call.args, policy.timeout_s)?;
        self.emit(ProxyEvent::LiveCall {
            server: call.server.clone(),
            tool: call.tool.clone(),
            args_hash,
        })?;
        // Validation, shaping or publication may fail after the remote effect.
        // Return that failure without replaying the already-dispatched mutation.
        self.finish_result(call, Some(policy), payload, Provenance::Live)
    }

    /// serveFrom: resolve `match` from args, find the item in the source
    /// tool's complete cache; `onMiss` governs absence.
    fn call_serve_from(&mut self, call: &ToolCall, policy: &ToolPolicy) -> Result<ToolResult> {
        let serve = policy.serve_from.as_ref().expect("checked by dispatch");
        let (src_server, src_tool) = split_policy_key(&serve.fetched_by).ok_or_else(|| {
            anyhow!(
                "serveFrom.fetchedBy '{}' is not <server>.<tool>",
                serve.fetched_by
            )
        })?;
        self.server(src_server)?.require_credentials()?;
        let match_value = call
            .args
            .get(&serve.match_field)
            .cloned()
            .ok_or_else(|| anyhow!("args missing match field '{}'", serve.match_field))?;

        for entry in self.cache.fresh_entries_for_tool(src_server, src_tool)? {
            for item in self.measured("cache", || {
                self.cache.read_items(src_server, src_tool, &entry.hash)
            })? {
                if item.get(&serve.match_field) == Some(&match_value) {
                    self.emit(ProxyEvent::ServedFromFile {
                        server: call.server.clone(),
                        tool: call.tool.clone(),
                        fetched_by: serve.fetched_by.clone(),
                        match_value: match_value.to_string(),
                    })?;
                    let result =
                        self.finish_result(call, Some(policy), item, Provenance::ServedFromFile)?;
                    self.server(src_server)?.require_credentials()?;
                    return Ok(result);
                }
            }
        }

        match policy.on_miss {
            Some(OnMiss::FetchLive) => {
                let args_hash = CacheStore::hash_args(&call.args, None, None);
                let payload =
                    self.call_live(&call.server, &call.tool, &call.args, policy.timeout_s)?;
                self.emit(ProxyEvent::LiveCall {
                    server: call.server.clone(),
                    tool: call.tool.clone(),
                    args_hash,
                })?;
                self.finish_result(call, Some(policy), payload, Provenance::Live)
            }
            Some(OnMiss::Fail) | None => bail!(
                "serveFrom miss for {}.{} ({} = {}) and onMiss != fetch_live",
                call.server,
                call.tool,
                serve.match_field,
                match_value
            ),
        }
    }

    /// Fetch the complete bounded dataset before returning its requested page.
    /// Completeness is required by the workflow's worklist artifact contract.
    fn call_paginated(&mut self, call: &ToolCall, policy: &ToolPolicy) -> Result<ToolResult> {
        const MAX_PAGES: u32 = 256;
        const MAX_ITEMS: usize = 100_000;
        const MAX_BYTES: usize = 32 * 1024 * 1024;
        let paginate = policy.paginate.as_ref().expect("checked by dispatch");
        let cache_policy = policy.cache.as_ref().expect("checked by dispatch");
        let param = &paginate.param;
        let hash = CacheStore::hash_args(&call.args, Some(&cache_policy.key), Some(param));
        let requested_page = call.args.get(param).and_then(Value::as_u64).unwrap_or(1);
        if requested_page == 0 {
            bail!("pagination page must be at least 1");
        }
        let cached = self
            .cache
            .fresh_entry(&call.server, &call.tool, &hash)?
            .is_some();
        if !cached {
            self.emit(ProxyEvent::CacheMiss {
                server: call.server.clone(),
                tool: call.tool.clone(),
                args_hash: hash.clone(),
            })?;
            let mut args = call.args.clone();
            args.as_object_mut()
                .context("paginated arguments must be an object")?
                .remove(param);
            let mut cursors = HashSet::new();
            let mut pages = Vec::new();
            let mut field = None;
            let mut count = 0usize;
            let mut bytes = 0usize;
            let started = std::time::Instant::now();
            loop {
                if pages.len() >= MAX_PAGES as usize
                    || started.elapsed() >= Duration::from_secs(120)
                {
                    bail!("pagination exceeded page or time limit");
                }
                self.validate_arguments(&call.server, &call.tool, &args)?;
                let remaining = Duration::from_secs(120).saturating_sub(started.elapsed());
                let timeout = self.bounded_timeout(
                    Duration::from_secs(policy.timeout_s.unwrap_or(30)).min(remaining),
                )?;
                let result = self.measured("transport", || {
                    self.server(&call.server)?.with_client(|client| {
                        let check = || {
                            if let Some(gate) = &self.send_gate {
                                gate()?;
                            }
                            self.bounded_timeout(timeout)?;
                            Ok(())
                        };
                        client.call_tool_checked(&call.tool, &args, Some(timeout), Some(&check))
                    })
                })?;
                let payload = extract_tool_payload(&result);
                self.response_size(
                    &call.server,
                    &call.tool,
                    "raw_page",
                    "live",
                    Some(crate::json_size::compact_bytes(&payload)?),
                    Some(pages.len() as u64 + 1),
                )?;
                if pages.is_empty() {
                    self.emit(ProxyEvent::LiveCall {
                        server: call.server.clone(),
                        tool: call.tool.clone(),
                        args_hash: hash.clone(),
                    })?;
                    self.emit(ProxyEvent::FetchStarted {
                        server: call.server.clone(),
                        tool: call.tool.clone(),
                        args_hash: hash.clone(),
                    })?;
                }
                let (page_field, items) =
                    find_items_field(&payload).context("paginated response has no items array")?;
                if let Some(previous) = &field {
                    if previous != &page_field {
                        bail!("pagination items field changed");
                    }
                } else {
                    field = Some(page_field);
                }
                count += items.len();
                bytes += crate::json_size::compact_bytes(&items)? as usize;
                if count > MAX_ITEMS || bytes > MAX_BYTES {
                    bail!("pagination exceeded item or byte limit");
                }
                pages.push(items);
                self.emit(ProxyEvent::FetchProgress {
                    server: call.server.clone(),
                    tool: call.tool.clone(),
                    args_hash: hash.clone(),
                    pages: pages.len() as u32,
                    items: count as u64,
                })?;
                let has_more = match paginate
                    .response_has_more
                    .as_deref()
                    .and_then(|pointer| payload.pointer(pointer))
                {
                    None => None,
                    Some(Value::Bool(value)) => Some(*value),
                    Some(_) => bail!("pagination responseHasMore must point to a boolean"),
                };
                if has_more == Some(false) {
                    break;
                }
                let cursor = match paginate.response_cursor.as_deref() {
                    Some(pointer) => payload.pointer(pointer),
                    None => payload.get(param),
                }
                .filter(|value| !value.is_null());
                if has_more == Some(true) && cursor.is_none() {
                    bail!("pagination hasMore is true but response cursor is missing or null");
                }
                match cursor {
                    None => break,
                    Some(cursor) => {
                        let key = serde_json::to_string(cursor)?;
                        if !cursors.insert(key) {
                            bail!("pagination cursor repeated");
                        }
                        args[param] = cursor.clone();
                    }
                }
            }
            if started.elapsed() >= Duration::from_secs(120) {
                bail!("pagination exceeded time limit");
            }
            let field = field.context("pagination has no field")?;
            self.cache.begin_paginated(
                &call.server,
                &call.tool,
                &hash,
                &field,
                &pages[0],
                cache_policy.ttl_s,
                &call.args,
            )?;
            for page in &pages[1..] {
                self.cache
                    .append_page(&call.server, &call.tool, &hash, page)?;
            }
            let meta = self.cache.finalize_paginated(
                &call.server,
                &call.tool,
                &hash,
                cache_policy.ttl_s,
                &call.args,
            )?;
            self.emit(ProxyEvent::FetchComplete {
                server: call.server.clone(),
                tool: call.tool.clone(),
                args_hash: hash.clone(),
                pages: meta.pages,
                total_items: meta.total_items.unwrap_or(0),
            })?;
        } else {
            self.emit(ProxyEvent::CacheHit {
                server: call.server.clone(),
                tool: call.tool.clone(),
                args_hash: hash.clone(),
            })?;
        }
        let (items, index, has_next, collection_bytes) = self
            .measured("cache", || {
                self.cache
                    .read_page_with_size(&call.server, &call.tool, &hash, requested_page)
            })?
            .context("requested pagination page is outside the completed dataset")?;
        let mut payload = json!({ index.field: items });
        payload[param] = if has_next {
            json!(requested_page + 1)
        } else {
            Value::Null
        };
        // Whole completed item array, distinct from upstream page envelopes and
        // the caller's selected page. Report this on cache hits as well.
        let source = if cached { "cache" } else { "live" };
        self.response_size(
            &call.server,
            &call.tool,
            "complete_collection",
            source,
            Some(collection_bytes),
            None,
        )?;
        self.response_size(
            &call.server,
            &call.tool,
            "selected_page",
            source,
            Some(crate::json_size::compact_bytes(&payload)?),
            Some(requested_page),
        )?;
        let mut artifacts = vec![self.publish_worklist(call, &hash)?];
        let content = match &policy.shape {
            Some(shape) => {
                self.shape_in_place(payload, shape, &call.server, &call.tool, &mut artifacts)?
            }
            None => payload,
        };
        self.server(&call.server)?.require_credentials()?;
        Ok(ToolResult {
            content_for_model: content,
            artifacts,
            provenance: if cached {
                Provenance::Cache
            } else {
                Provenance::Live
            },
        })
    }

    fn publish_worklist(&self, call: &ToolCall, hash: &str) -> Result<PathBuf> {
        let entry = self
            .cache
            .complete_entry(&call.server, &call.tool, hash)?
            .context("worklist cache incomplete")?;
        let items = self.cache.read_items(&call.server, &call.tool, hash)?;
        let path = self.artifact_dir.join(format!(
            "{}.{}.latest.jsonl",
            safe_path_component(&call.server),
            safe_path_component(&call.tool)
        ));
        let meta_path = PathBuf::from(format!("{}.meta.json", path.display()));
        // Invalidate the prior sidecar before replacing the data.
        write_file_atomic(
            &meta_path,
            &serde_json::to_vec(&json!({ "complete": false }))?,
        )?;
        let mut bytes = Vec::new();
        for item in items {
            serde_json::to_writer(&mut bytes, &item)?;
            bytes.push(b'\n');
        }
        write_file_atomic(&path, &bytes)?;
        write_file_atomic(&meta_path, &serde_json::to_vec(&entry.meta)?)?;
        Ok(path)
    }

    /// Simple cached read: hit → provenance Cache; miss → live call, fill cache.
    fn call_cached_read(&mut self, call: &ToolCall, policy: &ToolPolicy) -> Result<ToolResult> {
        let cache_policy = policy.cache.as_ref().expect("checked by dispatch");
        let hash = CacheStore::hash_args(&call.args, Some(&cache_policy.key), None);

        if self
            .cache
            .fresh_entry(&call.server, &call.tool, &hash)?
            .is_some()
        {
            let items = self.measured("cache", || {
                self.cache.read_items(&call.server, &call.tool, &hash)
            })?;
            if let Some(payload) = items.into_iter().next() {
                self.emit(ProxyEvent::CacheHit {
                    server: call.server.clone(),
                    tool: call.tool.clone(),
                    args_hash: hash,
                })?;
                return self.finish_result(call, Some(policy), payload, Provenance::Cache);
            }
        }

        self.emit(ProxyEvent::CacheMiss {
            server: call.server.clone(),
            tool: call.tool.clone(),
            args_hash: hash.clone(),
        })?;
        let payload = self.call_live(&call.server, &call.tool, &call.args, policy.timeout_s)?;
        self.emit(ProxyEvent::LiveCall {
            server: call.server.clone(),
            tool: call.tool.clone(),
            args_hash: hash.clone(),
        })?;
        if policy.result_validation.is_some() {
            // Reject before filling the cache, and invalidate a prior named
            // artifact even when this replacement cannot be admitted.
            self.prepare_result(call, Some(policy), &payload)?;
        }
        self.cache.write_single(
            &call.server,
            &call.tool,
            &hash,
            &payload,
            cache_policy.ttl_s,
            &call.args,
        )?;
        self.finish_result(call, Some(policy), payload, Provenance::Live)
    }

    /// Invalidate a replacement's sidecar before any admission check. Cache
    /// misses also use this before writing raw payloads into their cache.
    fn prepare_result(
        &self,
        call: &ToolCall,
        policy: Option<&ToolPolicy>,
        payload: &Value,
    ) -> Result<Option<PathBuf>> {
        self.measured("admission", || {
            self.require_tool_credentials(&call.server, &call.tool)?;
            let selected = self.prepared_for(call)?;
            let result_path = policy
                .and_then(|p| p.result_artifact.as_ref())
                .map(|name| {
                    let filename = if let Some(prepared) = selected {
                        crate::preparation::selected_result_artifact_name(name, &prepared.context)?
                    } else {
                        format!("{name}.json")
                    };
                    Ok::<_, anyhow::Error>(self.artifact_dir.join(filename))
                })
                .transpose()?;
            if let Some(path) = &result_path {
                if selected.is_some() {
                    match std::fs::symlink_metadata(path) {
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        _ => bail!("selected result artifact already exists or is unavailable"),
                    }
                }
                let meta_path = PathBuf::from(format!("{}.meta.json", path.display()));
                // A failed replacement must never leave new data marked complete.
                write_file_atomic(
                    &meta_path,
                    &serde_json::to_vec(&json!({"complete": false}))?,
                )?;
            }
            if let Some(validation) = policy.and_then(|p| p.result_validation.as_ref()) {
                self.result_validators
                    .get(&crate::policy::policy_key(&call.server, &call.tool))
                    .context("missing compiled tool result schema")?
                    .validate_result(payload, validation.max_bytes)?;
            }
            Ok(result_path)
        })
    }

    /// Preserve the parsed payload independently of the model-facing shape.
    fn finish_result(
        &self,
        call: &ToolCall,
        policy: Option<&ToolPolicy>,
        payload: Value,
        provenance: Provenance,
    ) -> Result<ToolResult> {
        self.measured("finish", || {
            if provenance != Provenance::Live {
                self.response_size(
                    &call.server,
                    &call.tool,
                    "raw_payload",
                    provenance_source(provenance),
                    Some(crate::json_size::compact_bytes(&payload)?),
                    None,
                )?;
            }
            let mut artifacts = Vec::new();
            let result_path = self.prepare_result(call, policy, &payload)?;
            if let Some(path) = &result_path {
                let bytes = serde_json::to_vec(&payload)?;
                if self.prepared_for(call)?.is_some() {
                    crate::cache::write_file_new_atomic(path, &bytes)?;
                } else {
                    write_file_atomic(path, &bytes)?;
                }
            }
            let content = match policy.and_then(|p| p.shape.as_ref()) {
                Some(shape) => {
                    self.shape_in_place(payload, shape, &call.server, &call.tool, &mut artifacts)?
                }
                None => payload,
            };
            if let Some(path) = result_path {
                let meta_path = PathBuf::from(format!("{}.meta.json", path.display()));
                write_file_atomic(
                    &meta_path,
                    &serde_json::to_vec(&json!({
                        "complete": true, "fetchedAt": crate::cache::now_secs(),
                    }))?,
                )?;
                artifacts.push(path);
            }
            self.require_tool_credentials(&call.server, &call.tool)?;
            Ok(ToolResult {
                content_for_model: content,
                artifacts,
                provenance,
            })
        })
    }

    /// Shape policy: strip named fields (recursively) out of the model-bound
    /// copy into artifact files (atomic write); the model receives
    /// {path, bytes, preview} per stripped field.
    fn shape_in_place(
        &self,
        mut payload: Value,
        shape: &ShapePolicy,
        server: &str,
        tool: &str,
        artifacts: &mut Vec<PathBuf>,
    ) -> Result<Value> {
        self.shape_walk(&mut payload, shape, server, tool, artifacts)?;
        Ok(payload)
    }

    fn shape_walk(
        &self,
        value: &mut Value,
        shape: &ShapePolicy,
        server: &str,
        tool: &str,
        artifacts: &mut Vec<PathBuf>,
    ) -> Result<()> {
        match value {
            Value::Object(map) => {
                let keys: Vec<String> = map.keys().cloned().collect();
                for key in keys {
                    if shape.strip.iter().any(|f| f == &key) {
                        let taken = map.remove(&key).expect("key present");
                        let replacement =
                            self.strip_to_artifact(&key, &taken, shape, server, tool, artifacts)?;
                        map.insert(key, replacement);
                    } else if let Some(child) = map.get_mut(&key) {
                        self.shape_walk(child, shape, server, tool, artifacts)?;
                    }
                }
            }
            Value::Array(items) => {
                for item in items.iter_mut() {
                    self.shape_walk(item, shape, server, tool, artifacts)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn strip_to_artifact(
        &self,
        field: &str,
        value: &Value,
        shape: &ShapePolicy,
        server: &str,
        tool: &str,
        artifacts: &mut Vec<PathBuf>,
    ) -> Result<Value> {
        let bytes = serde_json::to_vec_pretty(value)?;
        let byte_len = bytes.len() as u64;
        let preview = preview_of(value);
        if !shape.to_file {
            return Ok(json!({ "bytes": byte_len, "preview": preview }));
        }
        let digest = Sha256::digest(&bytes);
        let short: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
        let path = self
            .artifact_dir
            .join(safe_path_component(server))
            .join(safe_path_component(tool))
            .join(format!("{}-{short}.json", safe_path_component(field)));
        write_file_atomic(&path, &bytes)?;
        artifacts.push(path.clone());
        self.emit(ProxyEvent::ShapedToFile {
            server: server.to_string(),
            tool: tool.to_string(),
            field: field.to_string(),
            path: path.clone(),
            bytes: byte_len,
        })?;
        Ok(json!({ "path": path, "bytes": byte_len, "preview": preview }))
    }
}

fn is_local_tool(server: &str, tool: &str) -> bool {
    server == local::SERVER && local::supports(tool)
}

fn preview_of(value: &Value) -> String {
    const PREVIEW_LEN: usize = 160;
    let raw = match value {
        Value::String(s) => s.clone(),
        other => serde_json::to_string(other).unwrap_or_default(),
    };
    raw.chars().take(PREVIEW_LEN).collect()
}

fn provenance_source(provenance: Provenance) -> &'static str {
    match provenance {
        Provenance::Live => "live",
        Provenance::Cache => "cache",
        Provenance::ServedFromFile => "served_from_file",
        Provenance::Local => "local",
    }
}
