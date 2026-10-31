//! Scripted termination checks; no native inference or MCP server is used.
mod common;

use common::{read_events, ScriptGateway};
use trollm_core::{ExecutionPlan, Executor, RunDir, RunStatus, SlotMap, TaskRegistry};
use trollm_models::{FinishReason, GenerateRequest, GenerateResponse, ModelSlot};

struct ResponseModel {
    text: String,
    finish: FinishReason,
    capacity: Option<trollm_models::GenerationCapacity>,
    diagnostics: Option<trollm_models::GenerationDiagnostics>,
    fail: bool,
}
impl ModelSlot for ResponseModel {
    fn generate(&mut self, _: GenerateRequest) -> anyhow::Result<GenerateResponse> {
        if self.fail {
            return Err(trollm_models::ModelDiagnosticError::attach(
                anyhow::anyhow!("synthetic native prefill failure"),
                self.diagnostics.clone().unwrap(),
            ));
        }
        Ok(GenerateResponse {
            text: self.text.clone(),
            tokens_in: 1,
            tokens_out: 2048,
            finish: self.finish,
            capacity: self.capacity,
            diagnostics: self.diagnostics.clone(),
        })
    }
}

#[test]
fn length_never_accepts_prose_or_dispatches_partial_or_complete_fences() {
    for text in [
        "unfinished summary",
        "<tool_call>{\"server\":\"t\"",
        r#"<tool_call>{"server":"t","tool":"ping","args":{}}</tool_call>"#,
    ] {
        for completion in ["model-prose", "tool-result"] {
            let (status, events) = run(text, FinishReason::Length, completion);
            assert!(matches!(status, RunStatus::Failed { .. }));
            assert!(!events.iter().any(|e| e["type"] == "tool_call_result"));
            assert!(!events.iter().any(|e| e["type"] == "tool_call_intent"));
            let error = events.iter().find(|e| e["type"] == "error").unwrap();
            assert_eq!(error["class"], "model-error");
            assert!(error["detail"]
                .as_str()
                .unwrap()
                .contains("capacity exhausted"));
            let call = events.iter().find(|e| e["type"] == "model_call").unwrap();
            assert_eq!(call["finishReason"], "length");
            assert_eq!(call["tokensOut"], 2048);
        }
    }
}

#[test]
fn grammar_rejection_never_accepts_prose_or_dispatches_complete_fence() {
    for (text, completion) in [
        ("apparently complete prose", "model-prose"),
        (
            r#"<tool_call>{"server":"t","tool":"ping","args":{}}</tool_call>"#,
            "tool-result",
        ),
    ] {
        let (status, events) = run(text, FinishReason::GrammarReject, completion);
        assert!(matches!(status, RunStatus::Failed { .. }));
        assert!(!events.iter().any(|e| e["type"] == "tool_call_result"));
        let error = events.iter().find(|e| e["type"] == "error").unwrap();
        assert_eq!(error["class"], "model-error");
        assert!(error["detail"]
            .as_str()
            .unwrap()
            .contains("grammar rejected"));
        assert_eq!(
            events.iter().find(|e| e["type"] == "model_call").unwrap()["finishReason"],
            "grammar-reject"
        );
    }
}

#[test]
fn timeout_retains_partial_response_without_dispatch() {
    let (status, events) = run(
        "partial timeout response",
        FinishReason::Timeout,
        "tool-result",
    );
    assert!(matches!(status, RunStatus::Failed { .. }));
    assert!(!events
        .iter()
        .any(|event| event["type"] == "tool_call_intent"));
    assert_eq!(
        events
            .iter()
            .find(|event| event["type"] == "model_call")
            .unwrap()["finishReason"],
        "timeout"
    );
}

#[test]
fn stop_prose_and_tool_result_remain_successful() {
    for (text, completion) in [
        ("complete summary", "model-prose"),
        (
            r#"<tool_call>{"server":"t","tool":"ping","args":{}}</tool_call>"#,
            "tool-result",
        ),
    ] {
        let (status, events) = run(text, FinishReason::Stop, completion);
        assert_eq!(status, RunStatus::Succeeded);
        assert_eq!(
            events.iter().find(|e| e["type"] == "model_call").unwrap()["finishReason"],
            "stop"
        );
        assert_eq!(
            events
                .iter()
                .filter(|e| e["type"] == "tool_call_result")
                .count(),
            usize::from(completion == "tool-result")
        );
    }
}

#[test]
fn failure_diagnostic_is_bounded_and_cannot_replace_capacity_error() {
    for (text, budget) in [
        ("x".repeat(60 * 1024), 1),
        ("🦀é".repeat(10 * 1024), 1),
        ("partial".to_string(), 0),
    ] {
        let (status, events) = run_with_budget(&text, FinishReason::Length, "model-prose", budget);
        assert!(matches!(status, RunStatus::Failed { .. }));
        assert!(events.iter().any(|event| event["type"] == "error"
            && event["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains("capacity exhausted"))));
        assert!(!events
            .iter()
            .any(|event| event["type"] == "tool_call_intent"));
    }
}

fn run(text: &str, finish: FinishReason, completion: &str) -> (RunStatus, Vec<serde_json::Value>) {
    run_with_budget(text, finish, completion, 1)
}

fn run_with_budget(
    text: &str,
    finish: FinishReason,
    completion: &str,
    artifact_budget: u64,
) -> (RunStatus, Vec<serde_json::Value>) {
    run_configured(text, finish, completion, artifact_budget, None)
}

#[test]
fn length_diagnostic_distinguishes_requested_ceiling_from_context_capacity() {
    for (effective, expected) in [(16, "context capacity"), (2048, "requested output ceiling")] {
        let capacity = trollm_models::GenerationCapacity {
            context_tokens: 8192,
            context_remaining: effective,
            effective_max_tokens: effective,
        };
        let (_, events) = run_configured(
            "partial",
            FinishReason::Length,
            "tool-result",
            1,
            Some(capacity),
        );
        assert!(events.iter().any(|event| event["type"] == "error"
            && event["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains(expected))));
    }
}

fn run_configured(
    text: &str,
    finish: FinishReason,
    completion: &str,
    artifact_budget: u64,
    capacity: Option<trollm_models::GenerationCapacity>,
) -> (RunStatus, Vec<serde_json::Value>) {
    run_instrumented(
        text,
        finish,
        completion,
        artifact_budget,
        capacity,
        None,
        false,
    )
}

fn run_instrumented(
    text: &str,
    finish: FinishReason,
    completion: &str,
    artifact_budget: u64,
    capacity: Option<trollm_models::GenerationCapacity>,
    diagnostics: Option<trollm_models::GenerationDiagnostics>,
    fail: bool,
) -> (RunStatus, Vec<serde_json::Value>) {
    let base = common::tmp_dir("finish");
    std::fs::write(base.join("prompt.md"), "go").unwrap();
    let tool = completion == "tool-result";
    let mut plan = ExecutionPlan::from_yaml_str(
        r#"
schemaVersion: ledger.execution-plan.v1
id: finish
version: 1
input: { promptTemplate: go }
limits: { maxSteps: 2, maxReasonerCalls: 0, maxTokensTotal: 10000, timeoutS: 600, artifactMaxMb: 1 }
onLimit: abort
models: { coder: { ctx: 8192 } }
mcp: { servers: [], policies: {} }
entryStep: work
nodes:
  work: { kind: task, taskRef: work, onVerified: done }
  done: { kind: end, outcome: succeeded }
"#,
    )
    .unwrap();
    plan.limits.artifact_max_mb = artifact_budget;
    let registry = TaskRegistry::from_yaml_str(
        &format!(
            r#"
schemaVersion: ledger.task-registry.v1
tasks:
  work:
    model: coder
    prompt: prompt.md
    tools: {}
    completion: {completion}
    outputContract:
      outputs: {}
"#,
            if tool { "[t.ping]" } else { "[]" },
            if tool {
                "{}"
            } else {
                "{ summary: { from: model, type: prose } }"
            }
        ),
        &base,
    )
    .unwrap();
    let run = RunDir::under(&base);
    let path = run.path().to_path_buf();
    let mut exec = Executor::new(
        plan,
        registry,
        String::new(),
        SlotMap::new().with(
            "coder",
            ResponseModel {
                text: text.into(),
                finish,
                capacity,
                diagnostics: diagnostics.clone(),
                fail,
            },
        ),
        ScriptGateway::default(),
        run,
    )
    .unwrap();
    let status = exec.run_until_blocked().unwrap();
    let events = read_events(&path);
    let diagnostic_files: Vec<_> = std::fs::read_dir(path.join("artifacts"))
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("model-failure-")
                && path
                    .extension()
                    .is_some_and(|ext| ext == "json")
                    .then_some(())
                    .is_some()
                && !path.to_string_lossy().ends_with(".meta.json")
        })
        .collect();
    if fail {
        assert_eq!(diagnostic_files.len(), 1);
        let diagnostic: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&diagnostic_files[0]).unwrap()).unwrap();
        assert_eq!(diagnostic["responseAvailable"], false);
        assert!(diagnostic["response"].is_null());
        assert!(diagnostic["failure"]["ms"].is_u64());
        assert!(diagnostic["failure"]["tokensIn"].is_null());
        assert!(diagnostic["failure"]["tokensOut"].is_null());
        assert!(diagnostic["failure"]["finishReason"].is_null());
        assert_eq!(diagnostic["failure"]["diagnostics"]["prefillMs"], 0);
        assert!(diagnostic["failure"]["diagnostics"]["generationMs"].is_null());
        let calls: Vec<_> = events
            .iter()
            .filter(|event| event["type"] == "model_call")
            .collect();
        assert_eq!(calls.len(), 1);
        let call = calls[0];
        assert_eq!(call["failed"], true);
        assert!(call["ms"].is_u64());
        assert!(call["tokensIn"].is_null());
        assert!(call["tokensOut"].is_null());
        assert!(call["finishReason"].is_null());
        assert_eq!(call["diagnostics"], diagnostic["failure"]["diagnostics"]);
        assert_eq!(call["diagnostics"]["totalMs"], call["ms"]);
        assert!(call.get("response").is_none());
        assert!(call.get("text").is_none());
    } else if matches!(finish, FinishReason::Length | FinishReason::Timeout) && artifact_budget > 0
    {
        assert_eq!(diagnostic_files.len(), 1);
        let file_bytes = std::fs::metadata(&diagnostic_files[0]).unwrap().len();
        assert!(file_bytes <= 256 * 1024);
        assert_eq!(exec.state().counters.artifact_bytes, file_bytes);
        assert_eq!(exec.state().stack[0].counters.artifact_bytes, file_bytes);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&diagnostic_files[0])
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        let diagnostic: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&diagnostic_files[0]).unwrap()).unwrap();
        assert!(trollm_core::artifacts::is_complete(&diagnostic_files[0]));
        let mut end = text.len().min(48 * 1024);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        assert_eq!(diagnostic["response"]["text"]["text"], &text[..end]);
        assert_eq!(
            diagnostic["response"]["text"]["truncated"],
            text.len() > 48 * 1024
        );
        if diagnostics.is_some() {
            let call = events
                .iter()
                .find(|event| event["type"] == "model_call")
                .unwrap();
            assert_eq!(diagnostic["response"]["diagnostics"], call["diagnostics"]);
        }
        use sha2::Digest;
        let call = events
            .iter()
            .find(|event| event["type"] == "model_call")
            .unwrap();
        let request_prompt = diagnostic["request"]["prompt"]["text"].as_str().unwrap();
        assert_eq!(
            call["requestPromptSha256"],
            format!("{:x}", sha2::Sha256::digest(request_prompt.as_bytes()))
        );
        let grammar = diagnostic["request"]["grammarGbnf"]["text"].as_str();
        assert_eq!(
            call["grammarSha256"],
            serde_json::to_value(
                grammar.map(|text| format!("{:x}", sha2::Sha256::digest(text.as_bytes())))
            )
            .unwrap()
        );
        assert_eq!(diagnostic["response"]["tokensIn"], 1);
        assert_eq!(diagnostic["response"]["tokensOut"], 2048);
        assert_eq!(
            diagnostic["response"]["finishReason"],
            if finish == FinishReason::Timeout {
                "timeout"
            } else {
                "length"
            }
        );
        if let Some(capacity) = capacity {
            assert_eq!(
                diagnostic["response"]["capacity"]["effectiveMaxTokens"],
                capacity.effective_max_tokens
            );
            assert_eq!(
                diagnostic["response"]["lengthCause"],
                if capacity.effective_max_tokens < 2048 {
                    "context-capacity"
                } else {
                    "requested-output-ceiling"
                }
            );
        }
        assert_eq!(diagnostic["node"], "work");
        assert_eq!(diagnostic["task"], "work");
        assert!(diagnostic["request"]["prompt"]["text"]
            .as_str()
            .unwrap()
            .contains("go"));
        let expected = if tool {
            Some(
                trollm_models::fence::tool_call_grammar_with_argument_schemas(
                    &[("t".into(), "ping".into())],
                    &Default::default(),
                    trollm_models::fence::ToolCallGrammarMode::SingleToolCall,
                ),
            )
        } else {
            None
        };
        assert_eq!(
            diagnostic["request"]["grammarGbnf"]["text"].as_str(),
            expected.as_deref()
        );
        assert!(!events.iter().any(|event| event.to_string().contains(text)));
    } else {
        assert!(diagnostic_files.is_empty());
    }

    drop(exec);
    std::fs::remove_dir_all(base).unwrap();
    (status, events)
}

#[test]
fn measured_phases_survive_length_and_partial_prefill_timeout_without_dispatch() {
    for finish in [FinishReason::Length, FinishReason::Timeout] {
        let diagnostics = trollm_models::GenerationDiagnostics {
            prepare_ms: Some(0),
            prefill_ms: Some(0),
            generation_ms: (finish == FinishReason::Length).then_some(0),
            ..Default::default()
        };
        let (status, events) = run_instrumented(
            "partial",
            finish,
            "tool-result",
            1,
            None,
            Some(diagnostics),
            false,
        );
        assert!(matches!(status, RunStatus::Failed { .. }));
        let call = events
            .iter()
            .find(|event| event["type"] == "model_call")
            .unwrap();
        assert_eq!(call["diagnostics"]["totalMs"], call["ms"]);
        assert_eq!(call["diagnostics"]["prepareMs"], 0);
        assert_eq!(call["diagnostics"]["prefillMs"], 0);
        assert_eq!(
            call["diagnostics"]["generationMs"].is_null(),
            finish == FinishReason::Timeout
        );
        assert!(call["diagnostics"]["settings"]["backend"].is_null());
        assert!(!events
            .iter()
            .any(|event| event["type"] == "tool_call_intent"));
    }
}

#[test]
fn genuine_model_error_retains_only_available_timing_without_fabricated_response() {
    let diagnostics = trollm_models::GenerationDiagnostics {
        prepare_ms: Some(0),
        prefill_ms: Some(0),
        ..Default::default()
    };
    let (status, events) = run_instrumented(
        "not returned",
        FinishReason::Stop,
        "tool-result",
        1,
        None,
        Some(diagnostics),
        true,
    );
    assert!(matches!(status, RunStatus::Failed { .. }));
    assert!(!events
        .iter()
        .any(|event| event["type"] == "tool_call_intent"));
    assert!(events.iter().any(|event| event["type"] == "error"
        && event["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("synthetic native prefill failure"))));
}
