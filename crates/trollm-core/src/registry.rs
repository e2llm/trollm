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
#[serde(remote = "Self", rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskDef {
    /// How the request is produced. Omission preserves model execution.
    #[serde(default, skip_serializing_if = "TaskExecution::is_model")]
    pub execution: TaskExecution,
    /// Slot name only (§5); resolved against the plan's `models` slots.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::plan::deserialize_present_option"
    )]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation_timeout_s: Option<u64>,
    /// Path to the prompt template, relative to the registry file.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::plan::deserialize_present_option"
    )]
    pub prompt: Option<String>,
    /// Tool allowlist, entries `<server>.<tool>`; may be empty (§5).
    #[serde(default)]
    pub tools: Vec<String>,
    /// What completes this task: model prose, or one successful tool result.
    #[serde(default)]
    pub completion: TaskCompletion,
    /// Static destination for a single remote read's native result.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::plan::deserialize_present_option"
    )]
    pub result_artifact: Option<String>,
    /// Owned arguments resolved from frame variables, typed input keys or fixed JSON values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_arguments: Option<BTreeMap<String, TaskToolArguments>>,
    /// Coder-loop bound; omit for single-shot (§5).
    #[serde(rename = "loop")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_spec: Option<LoopSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mutations: Option<Mutations>,
    #[serde(default)]
    pub output_contract: OutputContract,
}

impl Serialize for TaskDef {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        TaskDef::serialize(self, serializer)
    }
}

impl<'de> Deserialize<'de> for TaskDef {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Preserve legacy nullable generation settings for model tasks while
        // rejecting their *presence*, including null, on authored tool tasks.
        let value = serde_yaml::Value::deserialize(deserializer)?;
        let execution = value
            .get("execution")
            .cloned()
            .map(TaskExecution::deserialize)
            .transpose()
            .map_err(serde::de::Error::custom)?
            .unwrap_or_default();
        if execution == TaskExecution::Tool {
            if value
                .get("mutations")
                .is_some_and(serde_yaml::Value::is_null)
            {
                return Err(serde::de::Error::custom(
                    "tool execution mutations must be an object when present",
                ));
            }
            for name in [
                "model",
                "prompt",
                "maxOutputTokens",
                "generationTimeoutS",
                "loop",
            ] {
                if value.get(name).is_some() {
                    return Err(serde::de::Error::custom(format!(
                        "tool execution forbids {name}"
                    )));
                }
            }
        }
        TaskDef::deserialize(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskExecution {
    #[default]
    Model,
    Tool,
}

impl TaskExecution {
    fn is_model(&self) -> bool {
        *self == Self::Model
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskCompletion {
    #[default]
    ModelProse,
    ToolResult,
    FlowRoute,
    /// One ordinary bound call, or one explicit flow.choose without success outputs.
    ToolOrRoute,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskToolArguments {
    /// Closed model-owned top-level names; absent retains the tool's openness.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::plan::deserialize_present_option"
    )]
    pub model_fields: Option<Vec<String>>,
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
    pub fn model_slot(&self) -> Option<&str> {
        (self.execution == TaskExecution::Model)
            .then_some(self.model.as_deref())
            .flatten()
    }

    pub fn prompt_path(&self) -> Option<&str> {
        (self.execution == TaskExecution::Model)
            .then_some(self.prompt.as_deref())
            .flatten()
    }

    fn validate_completion(&self) -> anyhow::Result<()> {
        match self.execution {
            TaskExecution::Model => {
                anyhow::ensure!(
                    self.model.as_ref().is_some_and(|s| !s.is_empty()),
                    "model execution requires a model slot"
                );
                anyhow::ensure!(
                    self.prompt.as_ref().is_some_and(|s| !s.is_empty()),
                    "model execution requires a prompt"
                );
            }
            TaskExecution::Tool => {
                anyhow::ensure!(
                    self.model.is_none() && self.prompt.is_none(),
                    "tool execution forbids model and prompt"
                );
                anyhow::ensure!(
                    self.max_output_tokens.is_none()
                        && self.generation_timeout_s.is_none()
                        && self.loop_spec.is_none(),
                    "tool execution forbids generation bounds and loop"
                );
                anyhow::ensure!(
                    self.completion == TaskCompletion::ToolResult
                        && self.tools.len() == 1
                        && !self.tools[0].starts_with("flow."),
                    "tool execution requires tool-result completion and exactly one ordinary tool"
                );
                anyhow::ensure!(
                    self.tool_arguments
                        .as_ref()
                        .is_some_and(|rules| rules.len() == 1
                            && rules.get(&self.tools[0]).is_some_and(|rule| rule
                                .model_fields
                                .as_ref()
                                .is_some_and(Vec::is_empty))),
                    "tool execution requires one matching toolArguments rule with modelFields: []"
                );
            }
        }
        anyhow::ensure!(
            self.model_slot() != Some(CANONICAL_REASONER_SLOT) || self.tools.is_empty(),
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
        if let Some(name) = &self.result_artifact {
            trollm_mcp::policy::validate_task_result_artifact_name(name)?;
            anyhow::ensure!(
                self.completion == TaskCompletion::ToolResult
                    && self.tools.len() == 1
                    && self.loop_spec.as_ref().map_or(1, |spec| spec.max_steps) == 1,
                "task resultArtifact requires single-tool, single-call tool-result completion"
            );
            let (server, _) = self.tools[0]
                .split_once('.')
                .ok_or_else(|| anyhow::anyhow!("task resultArtifact requires server.tool"))?;
            anyhow::ensure!(
                server != "local" && server != "flow",
                "task resultArtifact requires a remote read tool"
            );
        }
        for output in self.output_contract.outputs.values() {
            if let OutputSpec::FromArtifact {
                schema: Some(schema),
                ..
            }
            | OutputSpec::FromModel {
                schema: Some(schema),
                ..
            } = output
            {
                trollm_mcp::ArtifactSchema::compile(schema)?;
            }
            if let OutputSpec::FromArtifact {
                view: Some(view), ..
            } = output
            {
                view.validate()?;
            }
        }
        for tool in &self.tools {
            if tool.starts_with("flow.") {
                anyhow::ensure!(
                    tool == "flow.choose"
                        && matches!(
                            self.completion,
                            TaskCompletion::FlowRoute | TaskCompletion::ToolOrRoute
                        ),
                    "flow.choose requires flow-route completion; unknown flow tools are forbidden"
                );
            }
        }
        if self.completion == TaskCompletion::FlowRoute {
            anyhow::ensure!(
                self.model_slot() == Some("coder") && self.tools.iter().any(|t| t == "flow.choose"),
                "flow-route completion requires coder and flow.choose"
            );
        }
        if self.completion == TaskCompletion::ToolOrRoute {
            anyhow::ensure!(
                self.model_slot() == Some("coder")
                    && self.tools.iter().any(|tool| tool == "flow.choose")
                    && self.tools.iter().any(|tool| !tool.starts_with("flow.")),
                "tool-or-route requires coder, flow.choose and an ordinary tool"
            );
        }
        if let Some(bindings) = &self.tool_arguments {
            anyhow::ensure!(
                matches!(
                    self.completion,
                    TaskCompletion::ToolResult | TaskCompletion::ToolOrRoute
                ),
                "toolArguments requires tool-result completion"
            );
            anyhow::ensure!(!bindings.is_empty(), "toolArguments must not be empty");
            for (tool, rule) in bindings {
                anyhow::ensure!(
                    tool != "flow.choose",
                    "flow.choose arguments cannot be bound"
                );
                anyhow::ensure!(
                    tool.split('.').count() == 2
                        && tool.split('.').all(identifier)
                        && self.tools.contains(tool),
                    "toolArguments must name a granted server.tool"
                );
                anyhow::ensure!(
                    !(rule.from_vars.is_empty()
                        && rule.from_input.is_empty()
                        && rule.fixed.is_empty()
                        && rule.model_fields.is_none()),
                    "toolArguments requires bindings or modelFields"
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
                        identifier(argument) && path.split('.').all(|part| identifier(part) || (!part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))),
                        "fromVars requires argument names and dotted variable identifiers or unsigned array indices"
                    );
                }
                if let Some(fields) = &rule.model_fields {
                    for field in fields {
                        anyhow::ensure!(
                            identifier(field),
                            "modelFields requires argument identifiers"
                        );
                        anyhow::ensure!(
                            arguments.insert(field),
                            "modelFields has duplicate or bound argument names"
                        );
                    }
                }
            }
        }
        if matches!(
            self.completion,
            TaskCompletion::ToolResult | TaskCompletion::FlowRoute | TaskCompletion::ToolOrRoute
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
        /// Validate the trimmed model string without parsing it as JSON.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "crate::plan::deserialize_present_option"
        )]
        schema: Option<Value>,
    },
    FromArtifact {
        #[serde(rename = "fromArtifact")]
        from_artifact: String,
        /// Validate the complete original artifact before any projection.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "crate::plan::deserialize_present_option"
        )]
        schema: Option<Value>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "crate::plan::deserialize_present_option"
        )]
        view: Option<crate::artifact_view::ArtifactView>,
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
        let prompt = task
            .prompt_path()
            .ok_or_else(|| anyhow::anyhow!("task has no model prompt"))?;
        let path = base.join(prompt);
        std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("reading prompt {}: {e}", path.display()))
    }
}
