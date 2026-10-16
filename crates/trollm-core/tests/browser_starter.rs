//! Scripted starter wiring; this does not establish real-model/browser acceptance.
mod common;

use anyhow::{ensure, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use trollm_core::artifacts::{self, ArtifactMeta};
use trollm_core::{ExecutionPlan, Executor, RunDir, RunStatus, SlotMap, TaskRegistry};
use trollm_mcp::{Provenance, ToolCall, ToolDescriptor, ToolGateway, ToolResult};
use trollm_models::{GenerateRequest, GenerateResponse, MockModel, ModelSlot};

struct RecordingModel {
    model: MockModel,
    prompts: Arc<Mutex<Vec<String>>>,
}

impl ModelSlot for RecordingModel {
    fn generate(&mut self, request: GenerateRequest) -> Result<GenerateResponse> {
        self.prompts.lock().unwrap().push(request.prompt.clone());
        self.model.generate(request)
    }
}

struct BrowserGateway {
    artifacts: PathBuf,
    publish_page: bool,
    calls: Arc<Mutex<Vec<ToolCall>>>,
    page: Value,
}

impl ToolGateway for BrowserGateway {
    fn list_tools(&self) -> Result<Vec<ToolDescriptor>> {
        Ok(vec![])
    }

    fn validate_call(&self, call: &ToolCall) -> Result<()> {
        // These three calls are the entire scripted fixture's catalog.
        ensure!(call.server == "e2llm");
        ensure!(matches!(
            call.tool.as_str(),
            "act" | "list_tabs" | "read_page"
        ));
        Ok(())
    }

    fn call(&mut self, call: &ToolCall) -> Result<ToolResult> {
        self.calls.lock().unwrap().push(call.clone());
        let mut files = vec![];
        let content = match call.tool.as_str() {
            "act" => json!({"ok": true}),
            "list_tabs" => {
                json!({"tabs": [{"id": 17, "url": "https://example.org/", "title": "Fixture", "browser": "fixture"}]})
            }
            "read_page" => {
                if self.publish_page {
                    let written = artifacts::write_atomic(
                        &self.artifacts,
                        "page.json",
                        &serde_json::to_vec(&self.page)?,
                    )?;
                    artifacts::write_meta(
                        &written.path,
                        &ArtifactMeta {
                            fetched_at: 0,
                            complete: true,
                        },
                    )?;
                    files.push(written.path);
                }
                // Mirror the gateway's shaped response; page content stays in the artifact.
                json!({"url": "https://example.org/", "stats": {"contentNodes": 1, "truncated": false}})
            }
            _ => unreachable!(),
        };
        Ok(ToolResult {
            content_for_model: content,
            artifacts: files,
            provenance: Provenance::Live,
        })
    }
}

const OPEN: &str = r#"<tool_call>{"server":"e2llm","tool":"act","args":{"browser":"fixture","action":"open","target":"https://example.org/"}}</tool_call>"#;
const LIST: &str =
    r#"<tool_call>{"server":"e2llm","tool":"list_tabs","args":{"browser":"fixture"}}</tool_call>"#;
const READ: &str = r#"<tool_call>{"server":"e2llm","tool":"read_page","args":{"browser":"fixture","tabId":17}}</tool_call>"#;

fn run_fixture(
    open: bool,
    publish_page: bool,
    final_response: &str,
) -> (RunStatus, Vec<ToolCall>, Vec<String>, Value) {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../flows/starters/browser-summary");
    let plan = ExecutionPlan::from_yaml_file(&source.join("browser-summary.flow.yaml")).unwrap();
    let registry =
        TaskRegistry::from_yaml_file(&source.join("browser-summary.tasks.yaml")).unwrap();
    let base = common::tmp_dir("browser-starter");
    let run = RunDir::under(&base);
    let prompts = Arc::new(Mutex::new(vec![]));
    let calls = Arc::new(Mutex::new(vec![]));
    let page = json!({"title": "Fixture", "url": "https://example.org/", "content": "EXACT_ARTIFACT_PAGE", "stats": {"contentNodes": 1, "truncated": false}});
    let coder = if open {
        vec![OPEN, "Opened", LIST, READ, "CODER_PROSE_NOT_PAGE"]
    } else {
        vec!["No action was taken"]
    };
    let mut executor = Executor::new(
        plan,
        registry,
        json!({"browser": "fixture", "url": "https://example.org/", "question": "Summarize"})
            .to_string(),
        SlotMap::new()
            .with(
                "coder",
                RecordingModel {
                    model: MockModel::scripted(coder),
                    prompts: Arc::new(Mutex::new(vec![])),
                },
            )
            .with(
                "reasoner",
                RecordingModel {
                    model: MockModel::scripted([final_response]),
                    prompts: prompts.clone(),
                },
            ),
        BrowserGateway {
            artifacts: run.artifacts_dir(),
            publish_page,
            calls: calls.clone(),
            page: page.clone(),
        },
        run,
    )
    .unwrap();
    let status = executor.run_until_blocked().unwrap();
    let observed_calls = calls.lock().unwrap().clone();
    let observed_prompts = prompts.lock().unwrap().clone();
    if status == RunStatus::Succeeded {
        assert_eq!(
            executor.state().stack[0].vars["read_page"]["outputs"]["page"],
            page
        );
        assert_eq!(
            executor.state().stack[0].vars["summarize"]["outputs"]["report"],
            final_response
        );
    }
    drop(executor);
    std::fs::remove_dir_all(base).unwrap();
    (status, observed_calls, observed_prompts, page)
}

#[test]
fn browser_starter_hands_exact_page_to_tool_free_reasoner() {
    let (status, calls, prompts, page) =
        run_fixture(true, true, "Fixture summary: https://example.org/");
    assert_eq!(status, RunStatus::Succeeded);
    assert_eq!(
        calls.iter().map(|c| c.tool.as_str()).collect::<Vec<_>>(),
        ["act", "list_tabs", "read_page"]
    );
    assert_eq!(prompts.len(), 1);
    assert!(prompts[0].contains(&page.to_string()));
    assert!(!prompts[0].contains("CODER_PROSE_NOT_PAGE"));
}

#[test]
fn browser_starter_rejects_no_open_and_missing_page() {
    let (status, calls, prompts, _) = run_fixture(false, true, "Unused");
    assert!(matches!(status, RunStatus::Failed { .. }));
    assert!(calls.is_empty());
    assert!(prompts.is_empty());
    let (status, calls, prompts, _) = run_fixture(true, false, "Unused");
    assert!(matches!(status, RunStatus::Failed { .. }));
    assert_eq!(calls.len(), 3);
    assert!(prompts.is_empty());
}

#[test]
fn browser_starter_denies_reasoner_tool_dispatch() {
    let (status, calls, prompts, _) = run_fixture(true, true, LIST);
    assert!(matches!(status, RunStatus::Failed { .. }));
    assert_eq!(calls.len(), 3);
    assert_eq!(prompts.len(), 1);
}
