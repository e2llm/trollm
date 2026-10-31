//! Optional, content-pinned setup defaults. Runtime overrides are resolved elsewhere.

use anyhow::{bail, Context, Result};
use serde::{
    de::{MapAccess, Visitor},
    Deserialize, Deserializer, Serialize,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use trollm_core::plan::hex_sha256;
use trollm_core::ExecutionPlan;

pub const SETUP_FORMAT: &str = "trollm.flow-setup.v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FlowSetup {
    pub format: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<SetupInput>,
    pub models: BTreeMap<String, ModelSetup>,
    #[serde(default)]
    pub connections: BTreeMap<String, ConnectionSetup>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupInput {
    #[serde(default = "default_input_label")]
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    #[serde(
        default,
        deserialize_with = "input_fields",
        skip_serializing_if = "Option::is_none"
    )]
    pub fields: Option<Vec<InputField>>,
    #[serde(default, rename = "toolArguments")]
    pub tool_arguments: BTreeMap<String, ToolArguments>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InputField {
    pub key: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub kind: InputKind,
    #[serde(default = "default_input_length")]
    pub max_length: usize,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum InputKind {
    #[default]
    Text,
    Url,
}

fn input_fields<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<Vec<InputField>>, D::Error> {
    Vec::<InputField>::deserialize(deserializer).map(Some)
}

fn default_input_label() -> String {
    "Input".into()
}

fn default_input_length() -> usize {
    4096
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolArguments {
    #[serde(default)]
    pub from_input: BTreeMap<String, String>,
    #[serde(default)]
    pub fixed: BTreeMap<String, Value>,
    #[serde(default)]
    pub forbid: Vec<String>,
    #[serde(default)]
    pub require: Vec<String>,
}

pub(crate) struct ResolvedInput {
    pub canonical: String,
    pub argument_constraints: BTreeMap<String, Value>,
}

/// Read retained input evidence under its original independent byte bound.
pub(crate) fn read_input_evidence(directory: &Path, name: &str, maximum: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    fs::File::open(directory.join(name))?
        .take((maximum + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > maximum {
        bail!("retained input evidence exceeds bound");
    }
    Ok(bytes)
}

/// Native-authoritative normalization with an independently serialized APK value.
/// Equality is exact UTF-8 bytes, not parsed JSON equivalence.
pub(crate) fn admit_source_input(
    input: &SetupInput,
    raw: &str,
    apk_normalized: &str,
) -> Result<Value> {
    if input.fields.is_none() || apk_normalized.len() > 65536 {
        bail!("source admission requires bounded structured input");
    }
    let resolved = input.resolve(raw)?;
    if resolved.canonical != apk_normalized {
        bail!("APK/native input normalization or serialization disagreement");
    }
    Ok(json!({"schema":"trollm.source-input-admission.v1",
        "raw":{"sha256":hex_sha256(raw.as_bytes()),"sizeBytes":raw.len()},
        "normalized":{"sha256":hex_sha256(resolved.canonical.as_bytes()),"sizeBytes":resolved.canonical.len()},
        "basis":"raw-bytes","normalization":"rust-str-trim-unicode-scalars",
        "serialization":"serde-json-btree-string-object",
        "apkNormalized":apk_normalized}))
}

fn input_key(key: &str) -> bool {
    let bytes = key.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 32
        && (bytes[0].is_ascii_alphabetic() || bytes[0] == b'_')
        && bytes
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || *c == b'_')
}

fn argument_name(name: &str) -> bool {
    !name.is_empty() && name.chars().count() <= 128 && !name.chars().any(char::is_control)
}

impl SetupInput {
    fn validate(&self, servers: &BTreeMap<String, String>) -> Result<()> {
        let Some(fields) = &self.fields else {
            if !self.tool_arguments.is_empty() {
                bail!("toolArguments requires input fields");
            }
            return Ok(());
        };
        if fields.is_empty() || fields.len() > 8 {
            bail!("input fields must contain 1 to 8 entries");
        }
        let mut keys = BTreeSet::new();
        for field in fields {
            if !input_key(&field.key) || !keys.insert(field.key.as_str()) {
                bail!("input field keys must be unique ASCII identifiers of at most 32 characters");
            }
            if field.label.trim().is_empty() || !(1..=16384).contains(&field.max_length) {
                bail!("input fields need a label and maxLength between 1 and 16384");
            }
        }
        if self.tool_arguments.len() > 16 {
            bail!("toolArguments supports at most 16 tools");
        }
        for (tool, rule) in &self.tool_arguments {
            let Some((server, name)) = tool.split_once('.') else {
                bail!("toolArguments keys must be server.tool");
            };
            let builtin = server == trollm_mcp::local::SERVER && trollm_mcp::local::supports(name);
            let declared_remote = server != trollm_mcp::local::SERVER
                && server != "flow"
                && servers.contains_key(server);
            if !(builtin || declared_remote) || !argument_name(name) {
                bail!("toolArguments must name a declared server or a known built-in tool");
            }
            for name in rule
                .from_input
                .keys()
                .chain(rule.fixed.keys())
                .chain(rule.forbid.iter())
                .chain(rule.require.iter())
            {
                if !argument_name(name) {
                    bail!("tool argument names must be nonempty, at most 128 characters and contain no controls");
                }
            }
            if rule
                .from_input
                .values()
                .any(|key| !keys.contains(key.as_str()))
            {
                bail!("toolArguments references an unknown input field");
            }
            if rule
                .from_input
                .keys()
                .any(|key| rule.fixed.contains_key(key))
                || rule.forbid.iter().any(|key| {
                    rule.from_input.contains_key(key)
                        || rule.fixed.contains_key(key)
                        || rule.require.contains(key)
                })
            {
                bail!(
                    "toolArguments has conflicting bound, fixed, forbidden or required arguments"
                );
            }
        }
        Ok(())
    }

    pub(crate) fn resolve(&self, raw: &str) -> Result<ResolvedInput> {
        let Some(fields) = &self.fields else {
            return Ok(ResolvedInput {
                canonical: raw.to_owned(),
                argument_constraints: BTreeMap::new(),
            });
        };
        if raw.len() > 65536 {
            bail!("structured input exceeds 64 KiB");
        }
        // Never format serde errors: their diagnostics can include supplied values.
        // Decode the original object before collecting it: Value would silently
        // collapse duplicate names, including escaped aliases of the same key.
        struct StringObject;
        impl<'de> Visitor<'de> for StringObject {
            type Value = BTreeMap<String, String>;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("one object of unique string fields")
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut values = BTreeMap::new();
                while let Some(key) = map.next_key::<String>()? {
                    if values.contains_key(&key) {
                        return Err(serde::de::Error::custom("duplicate input field"));
                    }
                    values.insert(key, map.next_value::<String>()?);
                }
                Ok(values)
            }
        }
        let mut decoder = serde_json::Deserializer::from_str(raw);
        let object = decoder
            .deserialize_map(StringObject)
            .map_err(|_| anyhow::anyhow!("structured input requires unique string fields"))?;
        decoder
            .end()
            .map_err(|_| anyhow::anyhow!("invalid structured input suffix"))?;
        if object
            .keys()
            .any(|key| !fields.iter().any(|field| &field.key == key))
        {
            bail!("structured input contains an unknown field");
        }
        let mut normalized = BTreeMap::new();
        for field in fields {
            let text = match object.get(&field.key) {
                Some(value) => value.as_str(),
                None => "",
            }
            .trim();
            if (field.required && text.is_empty()) || text.chars().count() > field.max_length {
                bail!("input field {} is missing or exceeds maxLength", field.key);
            }
            if field.kind == InputKind::Url && !text.is_empty() {
                let (scheme, rest) = text
                    .split_once("://")
                    .context("input URL must start with http:// or https://")?;
                let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
                if !(scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"))
                    || authority.is_empty()
                    || text
                        .chars()
                        .any(|c| c.is_whitespace() || c.is_control() || c == '\\')
                {
                    bail!("input URL must use explicit HTTP(S) with a host and no whitespace or backslashes");
                }
                let url =
                    url::Url::parse(text).map_err(|_| anyhow::anyhow!("input URL is invalid"))?;
                if !matches!(url.scheme(), "http" | "https")
                    || url.host_str().is_none()
                    || !url.username().is_empty()
                    || url.password().is_some()
                    || text.split_once("://").is_some_and(|(_, rest)| {
                        rest.split(['/', '?', '#'])
                            .next()
                            .unwrap_or("")
                            .contains('@')
                    })
                {
                    bail!("input URL must use HTTP(S) with a host and no embedded credentials");
                }
            }
            normalized.insert(field.key.clone(), text.to_owned());
        }
        let canonical = serde_json::to_string(&normalized)?;
        if canonical.len() > 65536 {
            bail!("structured input exceeds 64 KiB");
        }
        let mut argument_constraints = BTreeMap::new();
        for (tool, rule) in &self.tool_arguments {
            let mut properties = BTreeMap::new();
            let mut required: BTreeSet<String> = rule.require.iter().cloned().collect();
            for (arg, value) in &rule.fixed {
                properties.insert(arg.clone(), json!({"const":value}));
                required.insert(arg.clone());
            }
            for (arg, key) in &rule.from_input {
                properties.insert(arg.clone(), json!({"const":normalized[key]}));
                required.insert(arg.clone());
            }
            let mut schema = json!({"type":"object", "properties":properties, "required":required});
            if !rule.forbid.is_empty() {
                schema["not"] = json!({"anyOf":rule.forbid.iter().map(|arg| json!({"required":[arg]})).collect::<Vec<_>>()});
            }
            argument_constraints.insert(tool.clone(), schema);
        }
        Ok(ResolvedInput {
            canonical,
            argument_constraints,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelSetup {
    pub label: String,
    pub url: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub ctx: u32,
    #[serde(default = "default_temperature")]
    pub temperature: f64,
    #[serde(default = "default_top_p")]
    pub top_p: f64,
}

fn default_temperature() -> f64 {
    0.2
}
fn default_top_p() -> f64 {
    0.95
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectionSetup {
    pub url: String,
    pub auth: ConnectionAuth,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issuer_url: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionAuth {
    Bearer,
    Oauth,
}

fn safe_key(value: &str) -> bool {
    let mut chars = value.bytes();
    chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}

fn https_url(value: &str) -> Result<()> {
    if !value.starts_with("https://") || value.chars().any(char::is_whitespace) {
        bail!("setup URLs must use HTTPS without whitespace");
    }
    let url = url::Url::parse(value).context("invalid setup URL")?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        bail!("setup URLs must use HTTPS with a host and no embedded credentials");
    }
    Ok(())
}

fn declarations(
    plan: &ExecutionPlan,
    models: &mut BTreeMap<String, u32>,
    servers: &mut BTreeMap<String, String>,
    is_root: bool,
) -> Result<()> {
    if !is_root && plan.setup.is_some() {
        bail!("setup is supported only on the root flow, not on subflows");
    }
    for (name, model) in &plan.models {
        if let Some(previous) = models.insert(name.clone(), model.ctx) {
            if previous != model.ctx {
                bail!("conflicting model slot context declarations in flow tree");
            }
        }
    }
    for server in &plan.mcp.servers {
        if server.name == "flow" {
            bail!("MCP server name flow is reserved for runtime control tools");
        }
        if server.name == trollm_mcp::local::SERVER {
            bail!("MCP server name local is reserved for built-in tools");
        }
        if let Some(previous) = servers.insert(server.name.clone(), server.url.clone()) {
            if previous != server.url {
                bail!("conflicting MCP server URL declarations in flow tree");
            }
        }
    }
    for child in plan.subflows.values() {
        declarations(child, models, servers, false)?;
    }
    Ok(())
}

impl FlowSetup {
    pub fn validate(&self, plan: &ExecutionPlan) -> Result<()> {
        if self.format != SETUP_FORMAT {
            bail!("unsupported setup format; expected {SETUP_FORMAT}");
        }
        if self.title.trim().is_empty()
            || self
                .input
                .as_ref()
                .is_some_and(|input| input.label.trim().is_empty())
        {
            bail!("setup title and input label must be nonempty");
        }
        let mut slots = BTreeMap::new();
        let mut servers = BTreeMap::new();
        declarations(plan, &mut slots, &mut servers, true)?;
        if !self.models.keys().eq(slots.keys()) {
            bail!("setup models must exactly cover the flow tree's declared slots");
        }
        for (slot, model) in &self.models {
            if !safe_key(slot) || model.label.trim().is_empty() {
                bail!("setup model keys must be safe names and labels nonempty");
            }
            https_url(&model.url)?;
            if model.sha256.len() != 64
                || !model
                    .sha256
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
            {
                bail!("setup model sha256 must be 64 lowercase hexadecimal characters");
            }
            if model.size_bytes == 0 || model.ctx == 0 || slots[slot] != model.ctx {
                bail!("setup model size must be positive and ctx must match its slot declaration");
            }
            if !model.temperature.is_finite()
                || !(0.0..=2.0).contains(&model.temperature)
                || !model.top_p.is_finite()
                || model.top_p <= 0.0
                || model.top_p > 1.0
            {
                bail!("setup sampling requires temperature in [0,2] and topP in (0,1]");
            }
        }
        if let Some(input) = &self.input {
            input.validate(&servers)?;
        }
        for (name, connection) in &self.connections {
            if name == "flow" {
                bail!("setup connection flow is reserved for runtime control tools");
            }
            if !safe_key(name) || servers.get(name) != Some(&connection.url) {
                bail!("setup connections must match declared server names and default URLs");
            }
            https_url(&connection.url)?;
            if let Some(issuer) = &connection.issuer_url {
                https_url(issuer)?;
            }
        }
        Ok(())
    }
}

/// Load only a manifest-verified descriptor whose raw bytes match the flow pin.
/// This is shared by installation, startup and setup inspection.
pub(crate) fn load_flow_setup(
    plan: &ExecutionPlan,
    bundle_root: &Path,
    verified: &BTreeMap<PathBuf, String>,
) -> Result<Option<FlowSetup>> {
    // Root-only setup is enforced even for a legacy root with no descriptor.
    let Some(reference) = &plan.setup else {
        fn no_subflow_setup(plan: &ExecutionPlan) -> Result<()> {
            for child in plan.subflows.values() {
                if child.setup.is_some() {
                    bail!("setup is supported only on the root flow, not on subflows");
                }
                no_subflow_setup(child)?;
            }
            Ok(())
        }
        no_subflow_setup(plan)?;
        return Ok(None);
    };
    reference.validate().map_err(anyhow::Error::msg)?;
    let root = bundle_root.canonicalize()?;
    let path = root
        .join(&reference.path)
        .canonicalize()
        .context("resolving setup descriptor")?;
    if !path.starts_with(&root) || !verified.contains_key(&path) {
        bail!("setup descriptor is outside the verified manifest");
    }
    let bytes = fs::read(&path).context("reading setup descriptor")?;
    if hex_sha256(&bytes) != reference.sha256 {
        bail!("setup descriptor sha256 does not match the root flow pin");
    }
    let descriptor: FlowSetup =
        serde_json::from_slice(&bytes).context("parsing setup descriptor JSON")?;
    descriptor.validate(plan)?;
    Ok(Some(descriptor))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn plan() -> ExecutionPlan {
        ExecutionPlan::from_yaml_str(
            r#"
schemaVersion: ledger.execution-plan.v1
id: setup
version: 1
limits: { maxSteps: 1, maxReasonerCalls: 0, maxTokensTotal: 100, timeoutS: 60, artifactMaxMb: 1 }
onLimit: abort
models: { coder: { ctx: 2048 } }
mcp: { servers: [{ name: browser, url: 'https://example.org/mcp' }] }
entryStep: done
nodes: { done: { kind: end, outcome: succeeded } }
"#,
        )
        .unwrap()
    }

    fn value() -> Value {
        json!({"format":SETUP_FORMAT,"title":"Summarize", "models":{"coder":{
            "label":"Coder","url":"https://example.org/model.gguf","sha256":"a".repeat(64),
            "sizeBytes":123,"ctx":2048}},"connections":{"browser":{
                "url":"https://example.org/mcp","auth":"oauth","issuerUrl":"https://example.org"}}})
    }

    fn structured_input() -> Value {
        json!({"fields":[
            {"key":"browser","label":"Browser","required":true,"maxLength":128},
            {"key":"url","label":"URL","kind":"url","required":true,"maxLength":2048},
            {"key":"question","label":"Question","maxLength":2048}],
            "toolArguments":{
                "browser.list_tabs":{"fromInput":{"browser":"browser"}},
                "browser.act":{"fromInput":{"browser":"browser","target":"url"},"fixed":{"action":"open"},"forbid":["sessionId","tabId"]},
                "browser.read_page":{"fromInput":{"browser":"browser"},"require":["tabId"],"forbid":["sessionId"]}}})
    }

    fn setup_with_input(input: Value) -> FlowSetup {
        let mut setup = value();
        setup["input"] = input;
        serde_json::from_value(setup).unwrap()
    }

    #[test]
    fn structured_input_defaults_canonicalization_and_resume_bindings() {
        let setup = setup_with_input(structured_input());
        setup.validate(&plan()).unwrap();
        let input = setup.input.unwrap();
        assert_eq!(input.label, "Input");
        assert_eq!(input.fields.as_ref().unwrap()[2].kind, InputKind::Text);
        let resolved = input
            .resolve(r#"{"url":" https://example.org/page ","browser":" chosen "}"#)
            .unwrap();
        assert_eq!(
            resolved.canonical,
            r#"{"browser":"chosen","question":"","url":"https://example.org/page"}"#
        );
        assert_eq!(
            resolved.argument_constraints["browser.act"]["properties"]["target"]["const"],
            "https://example.org/page"
        );
        let resumed = input.resolve(&resolved.canonical).unwrap();
        assert_eq!(resumed.canonical, resolved.canonical);
        assert_eq!(resumed.argument_constraints, resolved.argument_constraints);
        let legacy: SetupInput = serde_json::from_value(json!({"label":"Text"})).unwrap();
        let raw = "  plain old input, not JSON  ";
        assert_eq!(legacy.resolve(raw).unwrap().canonical, raw);
    }

    #[test]
    fn structured_input_can_constrain_builtin_without_a_connection() {
        let input = json!({
            "fields": [{"key": "limit", "label": "Budget", "required": true}],
            "toolArguments": {"local.calculate": {
                "fromInput": {"right": "limit"}, "fixed": {"operation": "compare"}
            }}
        });
        let setup = setup_with_input(input.clone());
        setup.validate(&plan()).unwrap();
        let resolved = setup.input.unwrap().resolve(r#"{"limit":"80"}"#).unwrap();
        assert_eq!(
            resolved.argument_constraints["local.calculate"]["properties"]["right"]["const"],
            "80"
        );
        let mut unknown = input.clone();
        unknown["toolArguments"] = json!({"local.unknown": {"fixed": {"operation": "compare"}}});
        assert!(setup_with_input(unknown).validate(&plan()).is_err());
        let mut conflicting = plan();
        conflicting.mcp.servers[0].name = "local".into();
        assert!(setup_with_input(input)
            .validate(&conflicting)
            .unwrap_err()
            .to_string()
            .contains("reserved"));
    }

    #[test]
    fn structured_input_can_constrain_decimal_extraction_without_a_connection() {
        let setup = setup_with_input(json!({
            "fields": [{"key": "text", "label": "Evidence", "required": true}],
            "toolArguments": {"local.extract_decimal": {
                "fromInput": {"primary": "text"},
                "fixed": {"secondary": "", "rules": {"prefix": "$", "fractionDigits": 2}}
            }}
        }));
        setup.validate(&plan()).unwrap();
        let resolved = setup.input.unwrap().resolve(r#"{"text":"$2.90"}"#).unwrap();
        assert_eq!(
            resolved.argument_constraints["local.extract_decimal"]["properties"]["primary"]
                ["const"],
            "$2.90"
        );
    }

    #[test]
    fn runtime_flow_namespace_cannot_be_a_setup_connection_or_argument_binding() {
        let mut descriptor = value();
        descriptor["connections"]["flow"] = json!({
            "url": "https://example.org/mcp", "auth": "bearer"
        });
        let setup: FlowSetup = serde_json::from_value(descriptor).unwrap();
        assert!(setup
            .validate(&plan())
            .unwrap_err()
            .to_string()
            .contains("reserved"));
        let mut remote_flow = plan();
        remote_flow.mcp.servers[0].name = "flow".into();
        let ordinary_setup: FlowSetup = serde_json::from_value(value()).unwrap();
        assert!(ordinary_setup
            .validate(&remote_flow)
            .unwrap_err()
            .to_string()
            .contains("reserved"));
        for tool in ["flow.choose", "flow.unknown"] {
            let setup = setup_with_input(json!({
                "fields": [{"key": "route", "label": "Route"}],
                "toolArguments": {tool: {"fromInput": {"route": "route"}}}
            }));
            assert!(setup.validate(&plan()).is_err());
        }
    }

    #[test]
    fn structured_input_rejects_bad_values_without_echoing_them() {
        let input = setup_with_input(structured_input()).input.unwrap();
        for raw in [
            r#"[]"#,
            r#"{"browser":"chosen"}"#,
            r#"{"browser":" ","url":"https://example.org"}"#,
            r#"{"browser":42,"url":"https://example.org"}"#,
            r#"{"browser":"chosen","url":"https://example.org","unknown":"secret-input"}"#,
            r#"{"browser":"chosen","url":"file:///tmp/private"}"#,
            r#"{"browser":"chosen","url":"https:///missing-host"}"#,
            r#"{"browser":"chosen","url":"https:example.org"}"#,
            r#"{"browser":"chosen","url":"https://?query=1"}"#,
            r#"{"browser":"chosen","url":"https://example.org/a b"}"#,
            r#"{"browser":"chosen","url":"https://exa\tmple.org"}"#,
            r#"{"browser":"chosen","url":"https://exa\nmple.org"}"#,
            r#"{"browser":"chosen","url":"https://example.org/\u0000"}"#,
            r#"{"browser":"chosen","url":"https://example.org\\path"}"#,
            r#"{"browser":"chosen","url":"https://example.org:bad-port/"}"#,
            r#"{"browser":"chosen","url":"https://secret-input@example.org"}"#,
            r#"{"browser":"chosen","url":"https://example.org","question":null}"#,
        ] {
            let error = input.resolve(raw).err().expect("bad input accepted");
            assert!(!format!("{error:#}").contains("secret-input"));
        }
        assert!(input.resolve(&" ".repeat(65537)).is_err());
        let upper = input
            .resolve(r#"{"browser":"chosen","url":"HTTPS://example.org:8443/path?q=1"}"#)
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&upper.canonical).unwrap()["url"],
            "HTTPS://example.org:8443/path?q=1"
        );
        let mut raw =
            json!({"browser":"chosen","url":"https://example.org","question":"é".repeat(2048)});
        input.resolve(&raw.to_string()).unwrap();
        raw["question"] = json!("é".repeat(2049));
        assert!(input.resolve(&raw.to_string()).is_err());
    }

    #[test]
    fn structured_input_rejects_invalid_definitions_and_conflicts() {
        for (pointer, bad) in [
            ("/fields", json!([])),
            ("/fields", json!(vec![json!({"key":"x","label":"X"}); 9])),
            ("/fields/0/key", json!("1bad")),
            ("/fields/0/key", json!("x".repeat(33))),
            ("/fields/1/key", json!("browser")),
            ("/fields/0/label", json!(" ")),
            ("/fields/0/maxLength", json!(0)),
            ("/fields/0/maxLength", json!(16385)),
            (
                "/toolArguments/browser.act/fromInput/browser",
                json!("unknown"),
            ),
            ("/toolArguments/browser.act/fixed/browser", json!("other")),
            ("/toolArguments/browser.act/forbid", json!(["browser"])),
            ("/toolArguments/browser.act/require", json!(["sessionId"])),
            ("/toolArguments/browser.act/require", json!(["bad\nname"])),
        ] {
            let mut input = structured_input();
            if pointer == "/toolArguments/browser.act/fixed/browser" {
                input["toolArguments"]["browser.act"]["fixed"]["browser"] = bad;
            } else if pointer.ends_with("/require") {
                input["toolArguments"]["browser.act"]["require"] = bad;
            } else {
                *input.pointer_mut(pointer).unwrap() = bad;
            }
            assert!(
                setup_with_input(input).validate(&plan()).is_err(),
                "accepted {pointer}"
            );
        }
        let mut input = structured_input();
        input["toolArguments"]["other.read"] = json!({});
        assert!(setup_with_input(input).validate(&plan()).is_err());
        input = structured_input();
        input.as_object_mut().unwrap().remove("fields");
        assert!(setup_with_input(input).validate(&plan()).is_err());
        for pointer in ["/fields/0", "/toolArguments/browser.act"] {
            let mut input = structured_input();
            input.pointer_mut(pointer).unwrap()["unknown"] = json!(true);
            assert!(serde_json::from_value::<SetupInput>(input).is_err());
        }
    }

    #[test]
    fn browser_constraints_reject_before_mutation_intent_without_dispatch() {
        use trollm_core::{Executor, RunDir, RunStatus, SlotMap, TaskRegistry};
        use trollm_mcp::{McpGateway, PolicySet, ServerConfig};
        use trollm_models::MockModel;
        let input = setup_with_input(structured_input()).input.unwrap();
        for args in [
            json!({"browser":"other","action":"open","target":"https://example.org/page"}),
            json!({"browser":"chosen","action":"navigate","target":"https://example.org/page"}),
            json!({"browser":"chosen","action":"open","target":"https://other.example/page"}),
            json!({"browser":"chosen","action":"open","target":"https://example.org/page","sessionId":"other-session"}),
        ] {
            let resolved = input
                .resolve(r#"{"browser":"chosen","url":"https://example.org/page"}"#)
                .unwrap();
            let root = std::env::temp_dir().join(format!(
                "trollm-input-constraints-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&root).unwrap();
            fs::write(root.join("open.md"), "Use {{ user_input }}").unwrap();
            let mut flow = plan();
            flow.entry_step = "open".into();
            // Parse nodes through the normal YAML path to avoid constructing graph internals.
            let source = serde_yaml::to_string(&flow).unwrap();
            let mut yaml: serde_yaml::Value = serde_yaml::from_str(&source).unwrap();
            yaml["nodes"] = serde_yaml::from_str("open: {kind: task, taskRef: open, onVerified: done}\ndone: {kind: end, outcome: succeeded}").unwrap();
            let flow =
                ExecutionPlan::from_yaml_str(&serde_yaml::to_string(&yaml).unwrap()).unwrap();
            let registry = TaskRegistry::from_yaml_str(
                r#"
schemaVersion: ledger.task-registry.v1
tasks:
  open:
    model: coder
    prompt: open.md
    tools: [browser.act]
    loop: {maxSteps: 2}
    mutations: {idempotency: intent-log, onUncertain: fail-run}
    outputContract: {outputs: {report: {from: model, type: prose}}}
"#,
                &root,
            )
            .unwrap();
            let run_dir = RunDir::under(&root);
            let gateway = McpGateway::new(
                vec![ServerConfig::new("browser", "https://example.invalid/mcp")],
                PolicySet::default(),
                root.join("cache"),
                run_dir.artifacts_dir(),
                None,
            )
            .unwrap()
            .with_argument_constraints(resolved.argument_constraints)
            .unwrap();
            let response = format!(
                "<tool_call>{}</tool_call>",
                json!({"server":"browser","tool":"act","args":args})
            );
            let mut executor = Executor::new(
                flow,
                registry,
                resolved.canonical,
                SlotMap::new().with("coder", MockModel::scripted([response])),
                gateway,
                run_dir.clone(),
            )
            .unwrap();
            assert!(matches!(
                executor.run_until_blocked().unwrap(),
                RunStatus::Failed { .. }
            ));
            let events = fs::read_to_string(run_dir.path().join("events.jsonl")).unwrap();
            assert!(events.contains("tool-call-parse"), "{events}");
            assert!(!events.contains("tool_call_intent"));
            assert!(!events.contains("tool_call_result"));
            drop(executor);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn defaults_and_legacy_absence_are_preserved() {
        let setup: FlowSetup = serde_json::from_value(value()).unwrap();
        setup.validate(&plan()).unwrap();
        assert_eq!(setup.models["coder"].temperature, 0.2);
        assert_eq!(setup.models["coder"].top_p, 0.95);
        assert_eq!(
            serde_json::to_value(&setup).unwrap()["format"],
            SETUP_FORMAT
        );
        assert!(load_flow_setup(
            &plan(),
            Path::new("not-needed-for-legacy"),
            &BTreeMap::new()
        )
        .unwrap()
        .is_none());
    }

    #[test]
    fn invalid_fields_types_sources_and_sampling_reject() {
        for (pointer, invalid) in [
            ("/format", json!("trollm.flow-setup.v2")),
            ("/title", json!("  ")),
            ("/models/coder/label", json!("")),
            ("/models/coder/url", json!("http://example.org/model")),
            (
                "/models/coder/url",
                json!("https://user:password@example.org/model"),
            ),
            ("/models/coder/sha256", json!("A".repeat(64))),
            ("/models/coder/sizeBytes", json!(0)),
            ("/models/coder/sizeBytes", json!("123")),
            ("/models/coder/ctx", json!(1024)),
            ("/models/coder/temperature", json!(2.1)),
            ("/models/coder/topP", json!(0)),
            ("/models/coder/topP", json!(1.1)),
            (
                "/connections/browser/url",
                json!("https://other.example/mcp"),
            ),
            ("/connections/browser/auth", json!("password")),
            (
                "/connections/browser/issuerUrl",
                json!("file:///etc/issuer"),
            ),
        ] {
            let mut value = value();
            // Insert optional sampling fields before using a JSON pointer.
            value["models"]["coder"]["temperature"] = json!(0.2);
            value["models"]["coder"]["topP"] = json!(0.95);
            *value.pointer_mut(pointer).unwrap() = invalid;
            let valid = serde_json::from_value::<FlowSetup>(value)
                .and_then(|setup| setup.validate(&plan()).map_err(serde::de::Error::custom));
            assert!(valid.is_err(), "accepted {pointer}");
        }
        for pointer in ["", "/models/coder", "/connections/browser"] {
            let mut v = value();
            v.pointer_mut(pointer).unwrap()["unknown"] = json!(true);
            assert!(serde_json::from_value::<FlowSetup>(v).is_err());
        }
        let mut unsafe_slot = value();
        let model = unsafe_slot["models"]["coder"].take();
        unsafe_slot["models"].as_object_mut().unwrap().clear();
        unsafe_slot["models"]["../coder"] = model;
        assert!(serde_json::from_value::<FlowSetup>(unsafe_slot)
            .unwrap()
            .validate(&plan())
            .is_err());
    }

    #[test]
    fn models_cover_tree_and_conflicting_declarations_reject() {
        let mut root = plan();
        let mut child = plan();
        child.id = "child".into();
        child.models.clear();
        child.models.insert(
            "reasoner".into(),
            trollm_core::plan::ModelSlotDecl { ctx: 4096 },
        );
        root.subflows.insert(child.id.clone(), child);
        let mut value = value();
        let setup: FlowSetup = serde_json::from_value(value.clone()).unwrap();
        assert!(setup.validate(&root).is_err());
        let mut model = value["models"]["coder"].clone();
        model["ctx"] = json!(4096);
        value["models"]["reasoner"] = model;
        let setup: FlowSetup = serde_json::from_value(value).unwrap();
        setup.validate(&root).unwrap();
        root.subflows.get_mut("child").unwrap().models.insert(
            "coder".into(),
            trollm_core::plan::ModelSlotDecl { ctx: 4096 },
        );
        assert!(setup.validate(&root).is_err());
        root.subflows
            .get_mut("child")
            .unwrap()
            .models
            .remove("coder");
        root.subflows.get_mut("child").unwrap().mcp.servers[0].url =
            "https://other.example/mcp".into();
        assert!(setup.validate(&root).is_err());
    }

    #[test]
    fn descriptor_must_be_a_verified_manifest_member() {
        let dir = std::env::temp_dir().join(format!(
            "trollm-setup-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let bytes = serde_json::to_vec(&value()).unwrap();
        fs::write(dir.join("setup.json"), &bytes).unwrap();
        let mut root = plan();
        root.setup = Some(trollm_core::plan::SetupRef {
            path: "setup.json".into(),
            sha256: hex_sha256(&bytes),
        });
        assert!(load_flow_setup(&root, &dir, &BTreeMap::new()).is_err());
        let verified = BTreeMap::from([(
            dir.join("setup.json").canonicalize().unwrap(),
            hex_sha256(&bytes),
        )]);
        load_flow_setup(&root, &dir, &verified).unwrap().unwrap();
        fs::write(dir.join("setup.json"), b"{}").unwrap();
        assert!(load_flow_setup(&root, &dir, &verified).is_err());
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn source_admission_rejects_duplicate_aliases_and_keeps_raw_identity() {
        let input = setup_with_input(structured_input()).input.unwrap();
        for raw in [
            r#"{"browser":"a","browser":"b","url":"https://example.org"}"#,
            r#"{"browser":"a","\u0062rowser":"b","url":"https://example.org"}"#,
            r#"{"browser":true,"url":"https://example.org"}"#,
            r#"{"browser":"a","url":"https://example.org"} {}"#,
        ] {
            assert!(input.resolve(raw).is_err());
        }
        let raw = r#"{"url":" https://example.org ","browser":" chosen "}"#;
        let normalized = input.resolve(raw).unwrap().canonical;
        let receipt = admit_source_input(&input, raw, &normalized).unwrap();
        assert_ne!(receipt["raw"]["sha256"], receipt["normalized"]["sha256"]);
        assert_eq!(receipt["raw"]["sizeBytes"], raw.len());
        assert!(admit_source_input(&input, raw, raw).is_err());
        assert!(admit_source_input(&input, &" ".repeat(65537), &normalized).is_err());
    }
}
