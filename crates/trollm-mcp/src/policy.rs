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
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub struct PaginatePolicy {
    /// Name of the request arg carrying the pagination cursor.
    pub param: String,
    pub deliver: Deliver,
    #[serde(rename = "backgroundFetch")]
    pub background_fetch: BackgroundFetch,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShapePolicy {
    #[serde(default)]
    pub strip: Vec<String>,
    #[serde(rename = "toFile", default)]
    pub to_file: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub struct RetryPolicy {
    /// Reserved transport setting; validation currently requires one attempt.
    #[serde(default = "default_attempts")]
    pub attempts: u32,
    /// Reserved transport setting; validation currently requires zero backoff.
    #[serde(rename = "backoffS", default)]
    pub backoff_s: u64,
}

fn default_attempts() -> u32 {
    1
}

/// Admission checks for an exact non-paginated payload, before shaping.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResultValidation {
    pub max_bytes: usize,
    pub schema: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolPolicy {
    #[serde(default = "default_class")]
    pub class: ToolClass,
    #[serde(default)]
    pub cache: Option<CachePolicy>,
    #[serde(default)]
    pub paginate: Option<PaginatePolicy>,
    #[serde(default)]
    pub shape: Option<ShapePolicy>,
    /// Publish the unshaped result as `<name>.json` plus complete metadata.
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
        for (key, policy) in &self.policies {
            if split_policy_key(key).is_some_and(|(server, _)| server == "flow") {
                bail!("policy '{key}': flow is reserved for runtime control tools");
            }
            if policy.serve_from.as_ref().is_some_and(|source| {
                split_policy_key(&source.fetched_by).is_some_and(|(server, _)| server == "flow")
            }) {
                bail!("policy '{key}': flow runtime control tools cannot be cache sources");
            }
            if let Some(retry) = &policy.retry {
                if retry.attempts != 1 || retry.backoff_s != 0 {
                    bail!("policy '{key}': transport retries are unsupported; omit retry or set retry.attempts to 1 and retry.backoffS to 0");
                }
            }
            if let Some(validation) = &policy.result_validation {
                if policy.paginate.is_some() {
                    bail!("resultValidation is only allowed for non-paginated policies");
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
                if policy.paginate.is_some() {
                    bail!("resultArtifact is only allowed for non-paginated policies");
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

#[cfg(test)]
mod authoring_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn retry_defaults_remain_valid_for_reads_and_mutations() {
        for class in ["read", "mutation"] {
            for retry in [
                None,
                Some(json!({})),
                Some(json!({"attempts": 1})),
                Some(json!({"backoffS": 0})),
                Some(json!({"attempts": 1, "backoffS": 0})),
            ] {
                let mut policy = json!({"class": class});
                if let Some(retry) = retry {
                    policy["retry"] = retry;
                }
                PolicySet::from_yaml_str(&json!({"custom.tool": policy}).to_string()).unwrap();
            }
        }
    }

    #[test]
    fn unsupported_retry_settings_are_rejected_for_reads_and_mutations() {
        for class in ["read", "mutation"] {
            for retry in [
                json!({"attempts": 0}),
                json!({"attempts": 2}),
                json!({"backoffS": 1}),
                json!({"attempts": 1, "backoffS": 3}),
            ] {
                let value = json!({"custom.tool": {"class": class, "retry": retry}});
                let error = PolicySet::from_yaml_str(&value.to_string())
                    .unwrap_err()
                    .to_string();
                assert!(error.contains("custom.tool"), "{error}");
                assert!(
                    error.contains("transport retries are unsupported"),
                    "{error}"
                );
                assert!(
                    error.contains("retry.attempts to 1 and retry.backoffS to 0"),
                    "{error}"
                );
            }
        }
    }

    #[test]
    fn gateway_revalidates_manually_constructed_retry_policies_before_setup() {
        use crate::{McpGateway, ServerConfig};
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };

        for class in ["read", "mutation"] {
            // Bypass the parsing helper, as public callers can construct PolicySet.
            let policies = PolicySet {
                policies: serde_yaml::from_str(
                    &json!({"custom.tool": {
                        "class": class, "retry": {"attempts": 2}
                    }})
                    .to_string(),
                )
                .unwrap(),
            };
            let token_lookups = Arc::new(AtomicUsize::new(0));
            let lookups = token_lookups.clone();
            let server = ServerConfig::new("custom", "https://example.invalid/mcp")
                .with_token_getter(Arc::new(move || {
                    lookups.fetch_add(1, Ordering::SeqCst);
                    None
                }));
            let error = McpGateway::new(
                vec![server],
                policies,
                Default::default(),
                Default::default(),
                None,
            )
            .err()
            .expect("unsupported retry must fail gateway construction");
            assert!(error
                .to_string()
                .contains("transport retries are unsupported"));
            assert_eq!(token_lookups.load(Ordering::SeqCst), 0);
        }
    }

    #[test]
    fn closed_policy_objects_reject_optional_key_typos() {
        for policy in [
            json!({"timoutS": 3}),
            json!({"cache": {"ttlS": 3, "keys": ["id"]}}),
            json!({"paginate": {"param": "cursor", "deliver": "first_page", "backgroundFetch": "all", "typo": true}}),
            json!({"shape": {"toFil": true}}),
            json!({"serveFrom": {"fetchedBy": "custom.list", "match": "id", "typo": true}}),
            json!({"retry": {"backofS": 2}}),
        ] {
            let value = json!({"custom_server.custom_tool": policy});
            let error = PolicySet::from_yaml_str(&value.to_string()).unwrap_err();
            assert!(format!("{error:#}").contains("unknown field"), "{error:#}");
        }
    }

    #[test]
    fn result_schemas_and_argument_names_remain_dynamic() {
        let value = json!({"custom_server.custom_tool": {
            "cache": {"key": ["custom_argument"], "ttlS": 30},
            "resultValidation": {"maxBytes": 4096, "schema": {
                "type": "object", "properties": {"custom_result": {"type": "string"}},
                "additionalProperties": false, "custom_annotation": "permitted"
            }}
        }});
        PolicySet::from_yaml_str(&value.to_string()).unwrap();
    }
}
