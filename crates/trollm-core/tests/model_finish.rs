//! Scripted termination checks; no native inference or MCP server is used.
mod common;

use common::{read_events, ScriptGateway};
use trollm_core::{ExecutionPlan, Executor, RunDir, RunStatus, SlotMap, TaskRegistry};
use trollm_models::{FinishReason, GenerateRequest, GenerateResponse, ModelSlot};

struct ResponseModel {
    text: String,
    finish: FinishReason,
}
impl ModelSlot for ResponseModel {
    fn generate(&mut self, _: GenerateRequest) -> anyhow::Result<GenerateResponse> {
        Ok(GenerateResponse {
            text: self.text.clone(),
            tokens_in: 1,
            tokens_out: 2048,
            finish: self.finish,
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

fn run(text: &str, finish: FinishReason, completion: &str) -> (RunStatus, Vec<serde_json::Value>) {
    let base = common::tmp_dir("finish");
    std::fs::write(base.join("prompt.md"), "go").unwrap();
    let tool = completion == "tool-result";
    let plan = ExecutionPlan::from_yaml_str(
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
            },
        ),
        ScriptGateway::default(),
        run,
    )
    .unwrap();
    let status = exec.run_until_blocked().unwrap();
    let events = read_events(&path);
    drop(exec);
    std::fs::remove_dir_all(base).unwrap();
    (status, events)
}
