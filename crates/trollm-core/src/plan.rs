//! Serde model of `ledger.execution-plan.v1` (docs/ARCHITECTURE-ANDROID.md §3)
//! plus the plan loader that resolves `flowRef` subflows and pins sha256 (§4.4).

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const EXECUTION_PLAN_SCHEMA: &str = "ledger.execution-plan.v1";

/// Maximum authored delay for each onError retry in the interactive app.
pub const MAX_ON_ERROR_BACKOFF_S: u64 = 300;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionPlan {
    pub schema_version: String,
    pub id: String,
    pub version: u32,
    /// Optional recorded-source provenance; direct authors may omit it.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_source_baseline"
    )]
    pub source_baseline: Option<SourceBaseline>,
    /// Optional bundle-relative setup descriptor, pinned by the root flow bytes.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_setup"
    )]
    pub setup: Option<SetupRef>,
    #[serde(default)]
    pub input: PlanInput,
    pub limits: Limits,
    pub on_limit: RunLimitPolicy,
    /// Slot declarations only (§3): the plan may name slots and ctx, never weights.
    pub models: BTreeMap<String, ModelSlotDecl>,
    #[serde(default)]
    pub mcp: McpSection,
    pub entry_step: String,
    pub nodes: BTreeMap<String, Node>,

    /// Plans referenced via `flowRef`, keyed by plan id. Not part of the YAML
    /// format; populated by the loader (`from_yaml_file`). flowRef frames need
    /// the referenced plan at push time (§4.4).
    #[serde(skip)]
    pub subflows: BTreeMap<String, ExecutionPlan>,
    /// sha256 of the YAML bytes this plan was loaded from (content pinning, §4.4
    /// frame identity `<id>@sha256:<hex>`). Empty for programmatically built plans.
    #[serde(skip)]
    pub sha256: String,
    #[serde(skip)]
    pub source_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceBaseline {
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupRef {
    pub path: String,
    pub sha256: String,
}

impl SetupRef {
    pub fn validate(&self) -> Result<(), String> {
        if self.path.is_empty()
            || self.path.contains(['\\', ':', '\0'])
            || self
                .path
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
        {
            return Err("setup.path must be a safe bundle-relative path".into());
        }
        if self.sha256.len() != 64
            || !self
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("setup.sha256 must be 64 lowercase hexadecimal characters".into());
        }
        Ok(())
    }
}

fn deserialize_setup<'de, D>(deserializer: D) -> Result<Option<SetupRef>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    if !value.is_object() {
        return Err(serde::de::Error::custom("setup must be an object"));
    }
    let setup: SetupRef = serde_json::from_value(value).map_err(serde::de::Error::custom)?;
    setup.validate().map_err(serde::de::Error::custom)?;
    Ok(Some(setup))
}

pub(crate) fn deserialize_source_baseline<'de, D>(
    deserializer: D,
) -> Result<Option<SourceBaseline>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    // Absence uses the field default. JSON values preserve the required object
    // and string types without YAML's coercion of null or numbers to strings.
    let value = serde_json::Value::deserialize(deserializer)?;
    if !value.is_object() {
        return Err(serde::de::Error::custom("sourceBaseline must be an object"));
    }
    serde_json::from_value::<SourceBaseline>(value)
        .map(Some)
        .map_err(serde::de::Error::custom)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlanInput {
    /// Reserved and inactive; accepted for legacy plan compatibility. Generation
    /// uses the task registry's templates, with `user_input` bound at runtime.
    #[serde(default)]
    pub prompt_template: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Limits {
    pub max_steps: u32,
    pub max_reasoner_calls: u32,
    pub max_tokens_total: u64,
    pub timeout_s: u64,
    pub artifact_max_mb: u64,
}

/// Terminal run-level budget policy (§3). Closed enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunLimitPolicy {
    Abort,
    RespondPartial,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSlotDecl {
    pub ctx: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpSection {
    #[serde(default)]
    pub servers: Vec<McpServer>,
    /// Per-tool policies keyed `<server>.<tool>` (§6). Kept opaque here: the
    /// policy structs are consumed by trollm-mcp's PolicyEngine. Core also
    /// checks declared mutation classes when validating task journaling/retry.
    #[serde(default)]
    pub policies: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpServer {
    pub name: String,
    pub url: String,
}

/// A transition value (§4.2): a node id, or an inline terminal/pause directive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Transition {
    Node(String),
    Inline(InlineTransition),
}

/// New optional authored fields distinguish omission from explicit null.
pub(crate) fn deserialize_present_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    if value.is_null() {
        return Err(serde::de::Error::custom("field must not be null"));
    }
    serde_json::from_value(value)
        .map(Some)
        .map_err(serde::de::Error::custom)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InlineTransition {
    pub status: InlineStatus,
    #[serde(default)]
    pub reason: String,
    #[serde(
        rename = "resumeAt",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_option"
    )]
    pub resume_at: Option<String>,
}

/// Closed set per §4.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InlineStatus {
    Paused,
    Failed,
}

/// Binding reference like `{ from: setup.outputs.items }` or `{ from: item }`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FromBinding {
    pub from: String,
}

/// Node-level output binding (§3): from an artifact written during the task, or
/// absent when the registry `outputContract` alone defines the outputs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum NodeOutputBinding {
    FromArtifact {
        #[serde(rename = "fromArtifact")]
        from_artifact: String,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "crate::plan::deserialize_present_option"
        )]
        view: Option<crate::artifact_view::ArtifactView>,
        format: Option<String>,
    },
    FromModel {
        from: ModelOutputFrom,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelOutputFrom {
    Model,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NodeVerify {
    pub artifact_complete: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OnError {
    #[serde(default)]
    pub retry: u32,
    #[serde(default, rename = "backoffS")]
    pub backoff_s: u64,
    pub then: Transition,
}

/// flowRef invocation (§4.4): pushes a call frame for another plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FlowRef {
    pub flow_ref: String,
    #[serde(default)]
    pub inputs: BTreeMap<String, FromBinding>,
}

/// foreach output aggregation spec (§4.6).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ForeachCollect {
    pub collect: CollectFrom,
    pub index_by: String,
}

/// Closed per §4.6: only `body.outputs` exists in v1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CollectFrom {
    #[serde(rename = "body.outputs")]
    BodyOutputs,
}

/// Closed enumeration/ordering values (§3): v1 allows only these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Enumeration {
    Snapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Ordering {
    Source,
}

/// Closed node kinds (§4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeKind {
    Task,
    Foreach,
    End,
}

/// The `outputs` block: task nodes bind outputs from model prose or artifacts
/// (§3); foreach nodes declare aggregation specs (§4.6). Untagged because the
/// two shapes are disjoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NodeOutputs {
    Task(BTreeMap<String, NodeOutputBinding>),
    Foreach(BTreeMap<String, ForeachCollect>),
}

/// Node model. Fields for all kinds live on one struct with `Option`s; the
/// validator (validate.rs) enforces which fields are required/forbidden per
/// kind. Authored keys are closed; kind-specific validation stays descriptive.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Node {
    pub kind: NodeKind,

    // kind: task
    #[serde(default)]
    pub task_ref: Option<String>,
    #[serde(default)]
    pub outputs: Option<NodeOutputs>,
    #[serde(default)]
    pub verify: Option<NodeVerify>,
    #[serde(default)]
    pub on_verified: Option<Transition>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_option"
    )]
    pub routes: Option<BTreeMap<String, Transition>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_present_option"
    )]
    pub prompt_supplement: Option<String>,
    #[serde(default)]
    pub on_error: Option<OnError>,

    // kind: foreach
    #[serde(default)]
    pub items: Option<FromBinding>,
    #[serde(default)]
    pub item_key: Option<String>,
    #[serde(default)]
    pub bind_as: Option<String>,
    #[serde(default)]
    pub enumeration: Option<Enumeration>,
    #[serde(default)]
    pub ordering: Option<Ordering>,
    #[serde(default)]
    pub max_items: Option<u32>,
    #[serde(default)]
    pub on_limit: Option<Transition>,
    #[serde(default)]
    pub body: Option<FlowRef>,
    #[serde(default)]
    pub between: Option<FlowRef>,
    #[serde(default)]
    pub on_empty: Option<Transition>,
    #[serde(default)]
    pub on_complete: Option<Transition>,

    // kind: end
    #[serde(default)]
    pub outcome: Option<String>,
}

impl ExecutionPlan {
    /// Frame identity used in RunState stacks (§4.4): `<id>@sha256:<hex>`.
    pub fn pin(&self) -> String {
        format!("{}@sha256:{}", self.id, self.sha256)
    }

    /// Load a plan from YAML, pin its sha256, and recursively load every plan
    /// referenced via `flowRef` from sibling files named `<flowRef>.flow.yaml`.
    pub fn from_yaml_file(path: &Path) -> anyhow::Result<Self> {
        let mut seen = std::collections::BTreeSet::new();
        Self::load_recursive(path, &mut seen)
    }

    fn load_recursive(
        path: &Path,
        seen: &mut std::collections::BTreeSet<PathBuf>,
    ) -> anyhow::Result<Self> {
        let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let bytes = std::fs::read(path)
            .map_err(|e| anyhow::anyhow!("reading plan {}: {e}", path.display()))?;
        let sha256 = hex_sha256(&bytes);
        let mut plan: ExecutionPlan = serde_yaml::from_slice(&bytes)
            .map_err(|e| anyhow::anyhow!("parsing plan {}: {e}", path.display()))?;
        plan.sha256 = sha256;
        plan.source_path = Some(path.to_path_buf());

        if seen.insert(canon.clone()) {
            let dir = path.parent().unwrap_or_else(|| Path::new("."));
            let mut refs: Vec<String> = Vec::new();
            for node in plan.nodes.values() {
                if let Some(body) = &node.body {
                    refs.push(body.flow_ref.clone());
                }
                if let Some(between) = &node.between {
                    refs.push(between.flow_ref.clone());
                }
            }
            refs.sort();
            refs.dedup();
            for r in refs {
                if plan.subflows.contains_key(&r) {
                    continue;
                }
                let sub_path = dir.join(format!("{r}.flow.yaml"));
                let sub = Self::load_recursive(&sub_path, seen)
                    .map_err(|e| anyhow::anyhow!("flowRef '{r}' of plan '{}': {e}", plan.id))?;
                plan.subflows.insert(sub.id.clone(), sub);
            }
        }
        Ok(plan)
    }

    /// Parse a plan from a YAML string (tests / in-memory bundles). Subflows
    /// can be attached by inserting into `subflows`.
    pub fn from_yaml_str(yaml: &str) -> anyhow::Result<Self> {
        let mut plan: ExecutionPlan = serde_yaml::from_str(yaml)?;
        plan.sha256 = hex_sha256(yaml.as_bytes());
        Ok(plan)
    }

    /// Look up a plan by id across this plan and its loaded subflows.
    pub fn find_plan(&self, id: &str) -> Option<&ExecutionPlan> {
        if self.id == id {
            return Some(self);
        }
        self.subflows.values().find_map(|sub| sub.find_plan(id))
    }
}

pub fn hex_sha256(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex(&h.finalize())
}

pub fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}
