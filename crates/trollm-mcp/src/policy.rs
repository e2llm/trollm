//! Serde model for the per-tool policy map (§6), keyed `<server>.<tool>`.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolClass {
    Read,
    Mutation,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CachePolicy {
    /// Arg names participating in the cache hash. Empty = all args.
    #[serde(default)]
    pub key: Vec<String>,
    #[serde(rename = "ttlS")]
    pub ttl_s: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Deliver {
    FirstPage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackgroundFetch {
    All,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PaginatePolicy {
    /// Name of the request arg carrying the pagination cursor.
    pub param: String,
    pub deliver: Deliver,
    #[serde(rename = "backgroundFetch")]
    pub background_fetch: BackgroundFetch,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ShapePolicy {
    #[serde(default)]
    pub strip: Vec<String>,
    #[serde(rename = "toFile", default)]
    pub to_file: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServeFromPolicy {
    /// Source tool key, `<server>.<tool>`, whose complete cache feeds this tool.
    #[serde(rename = "fetchedBy")]
    pub fetched_by: String,
    /// Arg/field name used to match an item in the source cache.
    #[serde(rename = "match")]
    pub match_field: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnMiss {
    FetchLive,
    Fail,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RetryPolicy {
    /// Total attempts (>= 1). Transport-level only.
    #[serde(default = "default_attempts")]
    pub attempts: u32,
    #[serde(rename = "backoffS", default)]
    pub backoff_s: u64,
}

fn default_attempts() -> u32 {
    1
}

/// Admission checks for an exact ordinary-read payload, before shaping.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResultValidation {
    pub max_bytes: usize,
    pub schema: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ToolPolicy {
    #[serde(default = "default_class")]
    pub class: ToolClass,
    #[serde(default)]
    pub cache: Option<CachePolicy>,
    #[serde(default)]
    pub paginate: Option<PaginatePolicy>,
    #[serde(default)]
    pub shape: Option<ShapePolicy>,
    /// Publish the unshaped read payload as `<name>.json` plus complete metadata.
    #[serde(rename = "resultArtifact", default)]
    pub result_artifact: Option<String>,
    #[serde(rename = "resultValidation", default)]
    pub result_validation: Option<ResultValidation>,
    #[serde(rename = "serveFrom", default)]
    pub serve_from: Option<ServeFromPolicy>,
    #[serde(rename = "onMiss", default)]
    pub on_miss: Option<OnMiss>,
    #[serde(default)]
    pub passthrough: bool,
    #[serde(rename = "timeoutS", default)]
    pub timeout_s: Option<u64>,
    #[serde(default)]
    pub retry: Option<RetryPolicy>,
}

fn default_class() -> ToolClass {
    ToolClass::Read
}

/// Per-tool policy map, keyed `<server>.<tool>`, parsed from the plan's
/// `mcp.policies` YAML value.
#[derive(Debug, Clone, Default)]
pub struct PolicySet {
    pub policies: HashMap<String, ToolPolicy>,
}

impl PolicySet {
    pub fn from_yaml_value(value: &serde_yaml::Value) -> Result<Self> {
        let policies: HashMap<String, ToolPolicy> =
            serde_yaml::from_value(value.clone()).context("parsing mcp.policies into PolicySet")?;
        let set = Self { policies };
        set.validate()?;
        Ok(set)
    }

    pub fn from_yaml_str(yaml: &str) -> Result<Self> {
        let value: serde_yaml::Value = serde_yaml::from_str(yaml).context("parsing policy YAML")?;
        Self::from_yaml_value(&value)
    }

    pub fn get(&self, server: &str, tool: &str) -> Option<&ToolPolicy> {
        self.policies.get(&policy_key(server, tool))
    }

    pub(crate) fn validate(&self) -> Result<()> {
        for policy in self.policies.values() {
            if let Some(validation) = &policy.result_validation {
                if policy.class != ToolClass::Read || policy.paginate.is_some() {
                    bail!("resultValidation is only allowed for non-paginated read policies");
                }
                if validation.max_bytes == 0 || validation.max_bytes > 262_144 {
                    bail!("resultValidation.maxBytes must be between 1 and 262144");
                }
            }
            if let Some(name) = &policy.result_artifact {
                // Dots are reserved for worklists, snapshots and sidecars.
                if name.is_empty()
                    || name.len() > 64
                    || !name.as_bytes()[0].is_ascii_alphanumeric()
                    || !name
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
                {
                    bail!("resultArtifact must be a safe basename: 1–64 ASCII letters/digits, '_' or '-', starting with a letter/digit");
                }
                if policy.class != ToolClass::Read || policy.paginate.is_some() {
                    bail!("resultArtifact is only allowed for non-paginated read policies");
                }
            }
        }
        Ok(())
    }
}

pub fn policy_key(server: &str, tool: &str) -> String {
    format!("{server}.{tool}")
}

/// Splits a `<server>.<tool>` key on the first dot.
pub fn split_policy_key(key: &str) -> Option<(&str, &str)> {
    key.split_once('.')
}
