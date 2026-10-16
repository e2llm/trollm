//! Effective app-selected settings, stored with a run independently of defaults.
//! Credentials never belong in this file.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use trollm_core::ExecutionPlan;

pub const CONFIGURATION_FILE: &str = "configuration.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunConfiguration {
    pub version: u32,
    pub models: BTreeMap<String, ResolvedModel>,
    #[serde(default)]
    pub connections: BTreeMap<String, ResolvedConnection>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResolvedModel {
    pub path: String,
    pub sha256: String,
    pub ctx: u32,
    pub temperature: f64,
    pub top_p: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedConnection {
    pub url: String,
}

fn declared_slots(plan: &ExecutionPlan, slots: &mut BTreeMap<String, u32>) -> Result<()> {
    for (name, model) in &plan.models {
        if slots
            .insert(name.clone(), model.ctx)
            .is_some_and(|ctx| ctx != model.ctx)
        {
            bail!("conflicting model contexts for slot '{name}'");
        }
    }
    for child in plan.subflows.values() {
        declared_slots(child, slots)?;
    }
    Ok(())
}

fn declared_connections(
    plan: &ExecutionPlan,
    servers: &mut BTreeMap<String, String>,
) -> Result<()> {
    for server in &plan.mcp.servers {
        if servers
            .insert(server.name.clone(), server.url.clone())
            .is_some_and(|url| url != server.url)
        {
            bail!("conflicting MCP URLs for server '{}'", server.name);
        }
    }
    for child in plan.subflows.values() {
        declared_connections(child, servers)?;
    }
    Ok(())
}

impl RunConfiguration {
    /// Normalize missing endpoint overrides and validate supplied settings. Model
    /// compatibility is deliberately left to the loader, not predicted here.
    pub fn validate_and_resolve(&mut self, plan: &ExecutionPlan) -> Result<()> {
        if self.version != 1 {
            bail!("unsupported run configuration version");
        }
        let mut slots = BTreeMap::new();
        declared_slots(plan, &mut slots)?;
        if !self.models.keys().eq(slots.keys()) {
            bail!("run configuration must resolve every declared model slot exactly once");
        }
        for (name, model) in &self.models {
            if !Path::new(&model.path).is_absolute() {
                bail!("resolved model path for '{name}' must be absolute");
            }
            if model.sha256.len() != 64
                || !model
                    .sha256
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
            {
                bail!("resolved model '{name}' requires a lowercase SHA-256 digest");
            }
            if model.ctx == 0
                || !model.temperature.is_finite()
                || !(0.0..=2.0).contains(&model.temperature)
                || !model.top_p.is_finite()
                || model.top_p <= 0.0
                || model.top_p > 1.0
            {
                bail!("invalid context or sampling settings for model '{name}'");
            }
        }
        let mut servers = BTreeMap::new();
        declared_connections(plan, &mut servers)?;
        if self
            .connections
            .keys()
            .any(|name| !servers.contains_key(name))
        {
            bail!("run configuration names an undeclared MCP server");
        }
        for (name, url) in servers {
            self.connections
                .entry(name)
                .or_insert(ResolvedConnection { url });
        }
        for connection in self.connections.values() {
            let url = url::Url::parse(&connection.url).context("invalid resolved MCP URL")?;
            if url.scheme() != "https"
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.fragment().is_some()
            {
                bail!("resolved MCP URLs must use HTTPS without embedded credentials or fragments");
            }
        }
        Ok(())
    }

    /// Capture the old manually provisioned slot paths/hashes once for a new
    /// native run. Old snapshots without this sidecar retain legacy behavior.
    #[cfg(feature = "llama")]
    pub fn legacy(plan: &ExecutionPlan, base: &Path) -> Result<Self> {
        let mut slots = BTreeMap::new();
        declared_slots(plan, &mut slots)?;
        let models = slots
            .into_iter()
            .map(|(name, ctx)| {
                let path = base.join("models").join(format!("{name}.gguf"));
                let manifest =
                    fs::read_to_string(base.join("models").join(format!("{name}.gguf.sha256")))
                        .with_context(|| format!("no SHA-256 manifest for model slot '{name}'"))?;
                Ok((
                    name,
                    ResolvedModel {
                        path: path.to_string_lossy().into_owned(),
                        sha256: manifest
                            .split_whitespace()
                            .next()
                            .unwrap_or("")
                            .to_ascii_lowercase(),
                        ctx,
                        temperature: 0.2,
                        top_p: 0.95,
                    },
                ))
            })
            .collect::<Result<_>>()?;
        let mut configuration = Self {
            version: 1,
            models,
            connections: BTreeMap::new(),
        };
        configuration.validate_and_resolve(plan)?;
        Ok(configuration)
    }

    pub fn save(&self, run_dir: &Path) -> Result<()> {
        trollm_core::artifacts::write_atomic(
            run_dir,
            CONFIGURATION_FILE,
            &serde_json::to_vec_pretty(self)?,
        )?;
        Ok(())
    }

    pub fn load(run_dir: &Path) -> Result<Option<Self>> {
        match fs::read(run_dir.join(CONFIGURATION_FILE)) {
            Ok(bytes) => Ok(Some(
                serde_json::from_slice(&bytes).context("invalid persisted run configuration")?,
            )),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub fn apply_to_plan(&self, plan: &mut ExecutionPlan) {
        for (name, model) in &mut plan.models {
            model.ctx = self.models[name].ctx;
        }
        for server in &mut plan.mcp.servers {
            if let Some(connection) = self.connections.get(&server.name) {
                server.url = connection.url.clone();
            }
        }
        for child in plan.subflows.values_mut() {
            self.apply_to_plan(child);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn configuration_rejects_unknown_fields_and_credentials() {
        assert!(serde_json::from_value::<RunConfiguration>(json!({
            "version":1,"models":{},"bearerToken":"secret"
        }))
        .is_err());
    }

    #[test]
    fn model_settings_roundtrip_without_defaults_changing() {
        let value = json!({"version":1,"models":{"coder":{
            "path":"/models/pinned.gguf","sha256":"a".repeat(64),
            "ctx":4096,"temperature":0.3,"topP":0.9
        }},"connections":{"browser":{"url":"https://example.org/mcp"}}});
        let configuration: RunConfiguration = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(serde_json::to_value(configuration).unwrap(), value);
    }

    #[test]
    fn model_overrides_and_endpoint_defaults_are_resolved_before_persistence() {
        let source = include_str!("../../../flows/examples/minimal-summary.flow.yaml");
        let mut plan = ExecutionPlan::from_yaml_str(&source.replace(
            "servers: []",
            "servers: [{name: browser, url: 'https://example.org/mcp'}]",
        ))
        .unwrap();
        let mut configuration = RunConfiguration {
            version: 1,
            models: plan
                .models
                .keys()
                .map(|name| {
                    (
                        name.clone(),
                        ResolvedModel {
                            path: "/models/chosen.gguf".into(),
                            sha256: "a".repeat(64),
                            ctx: 1024,
                            temperature: 0.4,
                            top_p: 0.8,
                        },
                    )
                })
                .collect(),
            connections: BTreeMap::new(),
        };
        configuration.validate_and_resolve(&plan).unwrap();
        assert_eq!(
            configuration.connections["browser"].url,
            "https://example.org/mcp"
        );
        configuration.connections.get_mut("browser").unwrap().url =
            "https://override.example/mcp".into();
        configuration.validate_and_resolve(&plan).unwrap();
        configuration.apply_to_plan(&mut plan);
        assert!(plan.models.values().all(|model| model.ctx == 1024));
        assert_eq!(plan.mcp.servers[0].url, "https://override.example/mcp");
        let mut missing = configuration.clone();
        missing.models.clear();
        assert!(missing.validate_and_resolve(&plan).is_err());
        for invalid in [
            "http://example.org/mcp",
            "https://user:secret@example.org/mcp",
            "https://example.org/mcp#fragment",
        ] {
            let mut credentials = configuration.clone();
            credentials.connections.get_mut("browser").unwrap().url = invalid.into();
            assert!(credentials.validate_and_resolve(&plan).is_err());
        }
        configuration.connections.insert(
            "unknown".into(),
            ResolvedConnection {
                url: "https://example.org".into(),
            },
        );
        assert!(configuration.validate_and_resolve(&plan).is_err());
    }
}
