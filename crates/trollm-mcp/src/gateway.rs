//! McpGateway: the single choke point between models and remote MCP servers.
//! Applies the §6 per-tool policies (cache / paginate / serveFrom / shape /
//! mutation passthrough) on top of the streamable-HTTP transport.

use crate::cache::{write_file_atomic, CacheStore};
use crate::events::ProxyEvent;
use crate::policy::{split_policy_key, OnMiss, PolicySet, ShapePolicy, ToolClass, ToolPolicy};
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
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub type EventSink = Box<dyn FnMut(ProxyEvent) + Send>;

/// Static gateway config for one MCP server; the bearer token is resolved by
/// the app (not the plan) via the getter.
pub struct ServerConfig {
    pub name: String,
    pub url: String,
    pub token_getter: Option<TokenGetter>,
    grant_bound: bool,
    allow_loopback_http: bool,
    root_certificates: RootCertificates,
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
    fn require_credentials(&self) -> Result<()> {
        if self.config.grant_bound {
            required_grant_token(self.config.token_getter.as_ref())?;
        }
        Ok(())
    }
    /// Freeze descriptors and validators together for this gateway's captured
    /// credential context. A failed discovery never publishes a partial catalog.
    fn catalog(&self) -> Result<Arc<ToolCatalog>> {
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
            *guard = Some(if self.config.grant_bound {
                client.require_grant_credentials()
            } else {
                client
            });
        }
        f(guard.as_mut().expect("client just initialized"))
    }
}

pub struct McpGateway {
    servers: HashMap<String, ServerEntry>,
    policies: PolicySet,
    result_validators: HashMap<String, ToolSchema>,
    argument_constraints: HashMap<String, ToolSchema>,
    cache: CacheStore,
    artifact_dir: PathBuf,
    sink: Option<Arc<Mutex<EventSink>>>,
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
        let mut names = HashSet::new();
        for server in &servers {
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
            servers,
            policies,
            result_validators,
            argument_constraints: HashMap::new(),
            cache: CacheStore::new(cache_root),
            artifact_dir,
            sink: sink.map(|s| Arc::new(Mutex::new(s))),
        })
    }

    /// Additional immutable run-local restrictions, checked alongside catalog schemas.
    pub fn with_argument_constraints(
        mut self,
        constraints: std::collections::BTreeMap<String, Value>,
    ) -> Result<Self> {
        if constraints.len() > 16 {
            bail!("at most 16 tool argument constraints are supported");
        }
        let compiled = constraints
            .into_iter()
            .map(|(key, schema)| {
                let (server, tool) = key
                    .split_once('.')
                    .context("argument constraint key must be server.tool")?;
                if !self.servers.contains_key(server) || tool.is_empty() {
                    bail!("argument constraint must name a declared server and exact tool");
                }
                Ok((key, ToolSchema::compile(&schema)?))
            })
            .collect::<Result<HashMap<_, _>>>()?;
        self.argument_constraints = compiled;
        Ok(self)
    }

    pub fn set_event_sink(&mut self, sink: EventSink) {
        self.sink = Some(Arc::new(Mutex::new(sink)));
    }

    pub fn cache(&self) -> &CacheStore {
        &self.cache
    }

    fn emit(&self, event: ProxyEvent) {
        if let Some(sink) = &self.sink {
            if let Ok(mut sink) = sink.lock() {
                sink(event);
            }
        }
    }

    fn server(&self, name: &str) -> Result<&ServerEntry> {
        self.servers
            .get(name)
            .ok_or_else(|| anyhow!("unknown MCP server '{name}'"))
    }

    fn call_live(
        &self,
        server: &str,
        tool: &str,
        args: &Value,
        timeout_s: Option<u64>,
    ) -> Result<Value> {
        self.validate_arguments(server, tool, args)?;
        let timeout = timeout_s.map(Duration::from_secs);
        let result = self
            .server(server)?
            .with_client(|client| client.call_tool(tool, args, timeout))?;
        Ok(extract_tool_payload(&result))
    }

    fn validate_argument_constraints(&self, server: &str, tool: &str, args: &Value) -> Result<()> {
        if let Some(constraint) = self.argument_constraints.get(&format!("{server}.{tool}")) {
            constraint.validate(args)?;
        }
        Ok(())
    }

    fn validate_arguments(&self, server: &str, tool: &str, args: &Value) -> Result<()> {
        if !args.is_object() {
            bail!("MCP tool arguments must be an object");
        }
        self.validate_argument_constraints(server, tool, args)?;
        let catalog = self.server(server)?.catalog()?;
        let entry = catalog
            .get(tool)
            .context("unknown tool in MCP server catalog")?;
        entry.schema.validate(args)
    }
}

impl ToolGateway for McpGateway {
    fn list_tools(&self) -> Result<Vec<ToolDescriptor>> {
        let mut out = Vec::new();
        for entry in self.servers.values() {
            out.extend(
                entry
                    .catalog()?
                    .values()
                    .map(|tool| tool.descriptor.clone()),
            );
        }
        out.sort_by(|a, b| (&a.server, &a.name).cmp(&(&b.server, &b.name)));
        Ok(out)
    }

    fn validate_call(&self, call: &ToolCall) -> Result<()> {
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

    fn call(&mut self, call: &ToolCall) -> Result<ToolResult> {
        self.validate_call(call)?;
        let policy = self.policies.get(&call.server, &call.tool).cloned();
        match &policy {
            Some(p) if p.class == ToolClass::Mutation => self.call_mutation(call, p),
            Some(p) if p.serve_from.is_some() => self.call_serve_from(call, p),
            Some(p) if p.paginate.is_some() && p.cache.is_some() => self.call_paginated(call, p),
            Some(p) if p.cache.is_some() => self.call_cached_read(call, p),
            _ => {
                // No policy (or a read policy without any strategy): plain live call.
                let args_hash = CacheStore::hash_args(&call.args, None, None);
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
                });
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
        });
        // A lost response may follow an applied business effect. Never retry mutations.
        let payload = self.call_live(&call.server, &call.tool, &call.args, policy.timeout_s)?;
        self.emit(ProxyEvent::LiveCall {
            server: call.server.clone(),
            tool: call.tool.clone(),
            args_hash,
        });
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
            for item in self.cache.read_items(src_server, src_tool, &entry.hash)? {
                if item.get(&serve.match_field) == Some(&match_value) {
                    self.emit(ProxyEvent::ServedFromFile {
                        server: call.server.clone(),
                        tool: call.tool.clone(),
                        fetched_by: serve.fetched_by.clone(),
                        match_value: match_value.to_string(),
                    });
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
                });
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
            });
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
                let remaining = Duration::from_secs(120).saturating_sub(started.elapsed());
                let timeout = Duration::from_secs(policy.timeout_s.unwrap_or(30)).min(remaining);
                self.validate_arguments(&call.server, &call.tool, &args)?;
                let result = self
                    .server(&call.server)?
                    .with_client(|client| client.call_tool(&call.tool, &args, Some(timeout)))?;
                let payload = extract_tool_payload(&result);
                if pages.is_empty() {
                    self.emit(ProxyEvent::LiveCall {
                        server: call.server.clone(),
                        tool: call.tool.clone(),
                        args_hash: hash.clone(),
                    });
                    self.emit(ProxyEvent::FetchStarted {
                        server: call.server.clone(),
                        tool: call.tool.clone(),
                        args_hash: hash.clone(),
                    });
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
                bytes += serde_json::to_vec(&items)?.len();
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
                });
                match payload.get(param).filter(|v| !v.is_null()) {
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
            });
        } else {
            self.emit(ProxyEvent::CacheHit {
                server: call.server.clone(),
                tool: call.tool.clone(),
                args_hash: hash.clone(),
            });
        }
        let (items, index, has_next) = self
            .cache
            .read_page(&call.server, &call.tool, &hash, requested_page)?
            .context("requested pagination page is outside the completed dataset")?;
        let mut payload = json!({ index.field: items });
        payload[param] = if has_next {
            json!(requested_page + 1)
        } else {
            Value::Null
        };
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
        let path = self
            .artifact_dir
            .join(format!("{}.{}.latest.jsonl", call.server, call.tool));
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
            let items = self.cache.read_items(&call.server, &call.tool, &hash)?;
            if let Some(payload) = items.into_iter().next() {
                self.emit(ProxyEvent::CacheHit {
                    server: call.server.clone(),
                    tool: call.tool.clone(),
                    args_hash: hash,
                });
                return self.finish_result(call, Some(policy), payload, Provenance::Cache);
            }
        }

        self.emit(ProxyEvent::CacheMiss {
            server: call.server.clone(),
            tool: call.tool.clone(),
            args_hash: hash.clone(),
        });
        let payload = self.call_live(&call.server, &call.tool, &call.args, policy.timeout_s)?;
        self.emit(ProxyEvent::LiveCall {
            server: call.server.clone(),
            tool: call.tool.clone(),
            args_hash: hash.clone(),
        });
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
        self.server(&call.server)?.require_credentials()?;
        let result_path = policy
            .and_then(|p| p.result_artifact.as_ref())
            .map(|name| self.artifact_dir.join(format!("{name}.json")));
        if let Some(path) = &result_path {
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
    }

    /// Preserve the parsed payload independently of the model-facing shape.
    fn finish_result(
        &self,
        call: &ToolCall,
        policy: Option<&ToolPolicy>,
        payload: Value,
        provenance: Provenance,
    ) -> Result<ToolResult> {
        let mut artifacts = Vec::new();
        let result_path = self.prepare_result(call, policy, &payload)?;
        if let Some(path) = &result_path {
            write_file_atomic(path, &serde_json::to_vec(&payload)?)?;
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
        self.server(&call.server)?.require_credentials()?;
        Ok(ToolResult {
            content_for_model: content,
            artifacts,
            provenance,
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
            .join(server)
            .join(tool)
            .join(format!("{field}-{short}.json"));
        write_file_atomic(&path, &bytes)?;
        artifacts.push(path.clone());
        self.emit(ProxyEvent::ShapedToFile {
            server: server.to_string(),
            tool: tool.to_string(),
            field: field.to_string(),
            path: path.clone(),
            bytes: byte_len,
        });
        Ok(json!({ "path": path, "bytes": byte_len, "preview": preview }))
    }
}

fn preview_of(value: &Value) -> String {
    const PREVIEW_LEN: usize = 160;
    let raw = match value {
        Value::String(s) => s.clone(),
        other => serde_json::to_string(other).unwrap_or_default(),
    };
    raw.chars().take(PREVIEW_LEN).collect()
}
