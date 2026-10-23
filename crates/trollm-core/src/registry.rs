//! Serde model of `ledger.task-registry.v1` (docs/ARCHITECTURE-ANDROID.md §5).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const TASK_REGISTRY_SCHEMA: &str = "ledger.task-registry.v1";

/// Reserved v1 slot name whose calls consume the reasoner budget and grant no tools.
pub const CANONICAL_REASONER_SLOT: &str = "reasoner";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskRegistry {
    pub schema_version: String,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub version: u32,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::plan::deserialize_source_baseline"
    )]
    pub source_baseline: Option<crate::plan::SourceBaseline>,
    pub tasks: BTreeMap<String, TaskDef>,

    /// Directory the registry was loaded from; prompt templates are resolved
    /// relative to it (§5: `prompt: prompts/prepare-worklist.md`).
    #[serde(skip)]
    pub base_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskDef {
    /// Slot name only (§5); resolved against the plan's `models` slots.
    pub model: String,
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
    #[serde(default)]
    pub generation_timeout_s: Option<u64>,
    /// Path to the prompt template, relative to the registry file.
    pub prompt: String,
    /// Tool allowlist, entries `<server>.<tool>`; may be empty (§5).
    #[serde(default)]
    pub tools: Vec<String>,
    /// What completes this task: model prose, or one successful tool result.
    #[serde(default)]
    pub completion: TaskCompletion,
    /// Owned arguments resolved from frame variables, typed input keys or fixed JSON values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_arguments: Option<BTreeMap<String, TaskToolArguments>>,
    /// Coder-loop bound; omit for single-shot (§5).
    #[serde(rename = "loop")]
    #[serde(default)]
    pub loop_spec: Option<LoopSpec>,
    #[serde(default)]
    pub mutations: Option<Mutations>,
    #[serde(default)]
    pub output_contract: OutputContract,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskCompletion {
    #[default]
    ModelProse,
    ToolResult,
    FlowRoute,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskToolArguments {
    #[serde(default)]
    pub from_vars: BTreeMap<String, String>,
    /// Typed values from exact top-level keys in structured user input.
    #[serde(default)]
    pub from_input: BTreeMap<String, String>,
    #[serde(default)]
    pub fixed: BTreeMap<String, Value>,
}

fn identifier(value: &str) -> bool {
    let mut chars = value.bytes();
    matches!(chars.next(), Some(b'a'..=b'z' | b'A'..=b'Z' | b'_'))
        && chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'_' | b'-'))
}

impl TaskDef {
    fn validate_completion(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.model != CANONICAL_REASONER_SLOT || self.tools.is_empty(),
            "canonical reasoner model slot must not grant tools"
        );
        anyhow::ensure!(
            self.max_output_tokens
                .is_none_or(|n| (1..=2048).contains(&n)),
            "maxOutputTokens must be 1 through 2048"
        );
        anyhow::ensure!(
            self.generation_timeout_s
                .is_none_or(|n| (1..=600).contains(&n)),
            "generationTimeoutS must be 1 through 600"
        );
        for tool in &self.tools {
            if tool.starts_with("flow.") {
                anyhow::ensure!(
                    tool == "flow.choose" && self.completion == TaskCompletion::FlowRoute,
                    "flow.choose requires flow-route completion; unknown flow tools are forbidden"
                );
            }
        }
        if self.completion == TaskCompletion::FlowRoute {
            anyhow::ensure!(
                self.model == "coder" && self.tools.iter().any(|t| t == "flow.choose"),
                "flow-route completion requires coder and flow.choose"
            );
        }
        if let Some(bindings) = &self.tool_arguments {
            anyhow::ensure!(
                self.completion == TaskCompletion::ToolResult,
                "toolArguments requires tool-result completion"
            );
            anyhow::ensure!(!bindings.is_empty(), "toolArguments must not be empty");
            for (tool, rule) in bindings {
                anyhow::ensure!(
                    tool.split('.').count() == 2
                        && tool.split('.').all(identifier)
                        && self.tools.contains(tool),
                    "toolArguments must name a granted server.tool"
                );
                anyhow::ensure!(
                    !(rule.from_vars.is_empty()
                        && rule.from_input.is_empty()
                        && rule.fixed.is_empty()),
                    "toolArguments requires at least one fromVars, fromInput or fixed argument"
                );
                let mut arguments = std::collections::BTreeSet::new();
                for argument in rule
                    .from_vars
                    .keys()
                    .chain(rule.from_input.keys())
                    .chain(rule.fixed.keys())
                {
                    anyhow::ensure!(
                        identifier(argument),
                        "toolArguments requires argument identifiers"
                    );
                    anyhow::ensure!(
                        arguments.insert(argument),
                        "toolArguments has duplicate argument sources"
                    );
                }
                anyhow::ensure!(
                    rule.from_input.values().all(|key| identifier(key)),
                    "fromInput requires top-level input identifiers"
                );
                for (argument, path) in &rule.from_vars {
                    anyhow::ensure!(
                        identifier(argument) && path.split('.').all(identifier),
                        "fromVars requires argument names and dotted variable identifiers"
                    );
                }
            }
        }
        if matches!(
            self.completion,
            TaskCompletion::ToolResult | TaskCompletion::FlowRoute
        ) {
            anyhow::ensure!(
                !self.tools.is_empty(),
                "tool-result/flow-route completion requires tools"
            );
            anyhow::ensure!(
                self.completion == TaskCompletion::FlowRoute
                    || self
                        .loop_spec
                        .as_ref()
                        .map(|spec| spec.max_steps)
                        .unwrap_or(1)
                        == 1,
                "tool-result completion requires loop.maxSteps = 1"
            );
            anyhow::ensure!(
                self.completion == TaskCompletion::FlowRoute || !self.output_contract.allow_noop,
                "tool-result completion cannot allowNoop"
            );
            anyhow::ensure!(
                !self
                    .output_contract
                    .outputs
                    .values()
                    .any(|output| matches!(output, OutputSpec::FromModel { .. })),
                "tool-result/flow-route completion cannot declare model outputs"
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LoopSpec {
    pub max_steps: u32,
}

/// Mutation journaling policy (§6). Closed keys.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Mutations {
    pub idempotency: Idempotency,
    pub on_uncertain: OnUncertain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Idempotency {
    IntentLog,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OnUncertain {
    RefetchState,
    FailRun,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutputContract {
    #[serde(default)]
    pub outputs: BTreeMap<String, OutputSpec>,
    #[serde(default)]
    pub require_artifact_complete: Vec<String>,
    /// §4.3: a task whose contract accepts a no-op must say so.
    #[serde(default)]
    pub allow_noop: bool,
}

/// One declared output (§5). Untagged: either `{ from: model, type: prose }`
/// or `{ fromArtifact: <name>, format: jsonl, type: list }`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum OutputSpec {
    FromModel {
        from: ModelFrom,
        #[serde(rename = "type")]
        type_: OutputType,
    },
    FromArtifact {
        #[serde(rename = "fromArtifact")]
        from_artifact: String,
        format: ArtifactFormat,
        #[serde(rename = "type")]
        type_: OutputType,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelFrom {
    Model,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtifactFormat {
    Jsonl,
    Json,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputType {
    Prose,
    List,
    Object,
    String,
    Number,
    Bool,
}

impl TaskRegistry {
    pub fn from_yaml_file(path: &Path) -> anyhow::Result<Self> {
        let bytes = std::fs::read(path)
            .map_err(|e| anyhow::anyhow!("reading registry {}: {e}", path.display()))?;
        let mut reg: TaskRegistry = serde_yaml::from_slice(&bytes)
            .map_err(|e| anyhow::anyhow!("parsing registry {}: {e}", path.display()))?;
        reg.base_dir = path.parent().map(|p| p.to_path_buf());
        reg.validate_completion()?;
        Ok(reg)
    }

    /// Parse a registry from a YAML string; prompt paths resolve against
    /// `base_dir`.
    pub fn from_yaml_str(yaml: &str, base_dir: impl Into<PathBuf>) -> anyhow::Result<Self> {
        let mut reg: TaskRegistry = serde_yaml::from_str(yaml)?;
        reg.base_dir = Some(base_dir.into());
        reg.validate_completion()?;
        Ok(reg)
    }

    pub fn task(&self, task_ref: &str) -> Option<&TaskDef> {
        self.tasks.get(task_ref)
    }

    pub(crate) fn validate_completion(&self) -> anyhow::Result<()> {
        for (name, task) in &self.tasks {
            task.validate_completion()
                .map_err(|error| anyhow::anyhow!("task '{name}': {error}"))?;
        }
        Ok(())
    }

    /// Load and return the raw prompt template text for a task.
    pub fn prompt_template(&self, task: &TaskDef) -> anyhow::Result<String> {
        let base = self.base_dir.clone().unwrap_or_else(|| PathBuf::from("."));
        let path = base.join(&task.prompt);
        std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("reading prompt {}: {e}", path.display()))
    }
}
