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
    opened_complete: Option<bool>,
    publish_page: bool,
    calls: Arc<Mutex<Vec<ToolCall>>>,
    opened: Value,
    page: Value,
}

impl ToolGateway for BrowserGateway {
    fn tool_class(&self, _: &str, tool: &str) -> trollm_mcp::policy::ToolClass {
        if tool == "act" {
            trollm_mcp::policy::ToolClass::Mutation
        } else {
            trollm_mcp::policy::ToolClass::Read
        }
    }
    fn list_tools(&self) -> Result<Vec<ToolDescriptor>> {
        Ok(vec![])
    }

    fn validate_call(&self, call: &ToolCall) -> Result<()> {
        // These two calls are the entire scripted fixture's catalog.
        ensure!(call.server == "e2llm");
        ensure!(matches!(call.tool.as_str(), "act" | "read_page"));
        Ok(())
    }

    fn call(&mut self, call: &ToolCall) -> Result<ToolResult> {
        self.calls.lock().unwrap().push(call.clone());
        let mut files = vec![];
        let content = match call.tool.as_str() {
            "act" => {
                if let Some(complete) = self.opened_complete {
                    let written = artifacts::write_atomic(
                        &self.artifacts,
                        "opened.json",
                        &serde_json::to_vec(&self.opened)?,
                    )?;
                    artifacts::write_meta(
                        &written.path,
                        &ArtifactMeta {
                            fetched_at: 0,
                            complete,
                        },
                    )?;
                    files.push(written.path);
                }
                self.opened.clone()
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
const READ: &str =
    r#"<tool_call>{"server":"e2llm","tool":"read_page","args":{"browser":"fixture"}}</tool_call>"#;

struct FixtureResult {
    status: RunStatus,
    calls: Vec<ToolCall>,
    coder_prompts: Vec<String>,
    reasoner_prompts: Vec<String>,
    opened: Value,
    page: Value,
}

fn run_fixture(
    coder: &[&str],
    opened_complete: Option<bool>,
    publish_page: bool,
    final_response: &str,
) -> FixtureResult {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../flows/starters/browser-summary");
    let plan = ExecutionPlan::from_yaml_file(&source.join("browser-summary.flow.yaml")).unwrap();
    let registry =
        TaskRegistry::from_yaml_file(&source.join("browser-summary.tasks.yaml")).unwrap();
    let base = common::tmp_dir("browser-starter");
    let run = RunDir::under(&base);
    let prompts = Arc::new(Mutex::new(vec![]));
    let coder_prompts = Arc::new(Mutex::new(vec![]));
    let calls = Arc::new(Mutex::new(vec![]));
    let page = json!({"title": "Fixture", "url": "https://example.org/", "content": "EXACT_ARTIFACT_PAGE", "stats": {"contentNodes": 1, "truncated": false}});
    let opened =
        json!({"result": "opened", "tabId": 178, "url": "https://example.org/", "sessionId": ""});
    let mut executor = Executor::new(
        plan,
        registry,
        json!({"browser": "fixture", "url": "https://example.org/", "question": "Summarize"})
            .to_string(),
        SlotMap::new()
            .with(
                "coder",
                RecordingModel {
                    model: MockModel::scripted(coder.iter().copied()),
                    prompts: coder_prompts.clone(),
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
            opened_complete,
            publish_page,
            calls: calls.clone(),
            opened: opened.clone(),
            page: page.clone(),
        },
        run,
    )
    .unwrap();
    let status = executor.run_until_blocked().unwrap();
    let observed_calls = calls.lock().unwrap().clone();
    let observed_prompts = prompts.lock().unwrap().clone();
    let observed_coder_prompts = coder_prompts.lock().unwrap().clone();
    if status == RunStatus::Succeeded {
        assert_eq!(executor.state().stack[0].counters.steps, 3);
        assert_eq!(executor.state().stack[0].counters.reasoner_calls, 1);
        assert_eq!(
            executor.state().stack[0].vars["open_page"]["outputs"]["opened"],
            opened
        );
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
    FixtureResult {
        status,
        calls: observed_calls,
        coder_prompts: observed_coder_prompts,
        reasoner_prompts: observed_prompts,
        opened,
        page,
    }
}

#[test]
fn browser_starter_hands_exact_page_to_tool_free_reasoner() {
    let result = run_fixture(
        &[OPEN, READ],
        Some(true),
        true,
        "Fixture summary: https://example.org/",
    );
    assert_eq!(result.status, RunStatus::Succeeded);
    assert_eq!(
        result
            .calls
            .iter()
            .map(|c| c.tool.as_str())
            .collect::<Vec<_>>(),
        ["act", "read_page"]
    );
    assert_eq!(result.coder_prompts.len(), 2);
    // The model never selects or copies the ID; the typed binding supplies it.
    assert_eq!(result.calls[1].args["tabId"], result.opened["tabId"]);
    assert_eq!(result.calls[1].args["tabId"], json!(178));
    assert!(!result.coder_prompts[1].contains("178"));
    assert!(!result.coder_prompts[1].contains("list_tabs"));
    assert_eq!(result.reasoner_prompts.len(), 1);
    assert!(result.reasoner_prompts[0].contains(&result.page.to_string()));
    assert!(!result.reasoner_prompts[0].contains("tabId"));
}

#[test]
fn browser_starter_rejects_no_open_and_missing_page() {
    let result = run_fixture(&["No action was taken"], Some(true), true, "Unused");
    assert!(matches!(result.status, RunStatus::Failed { .. }));
    assert!(result.calls.is_empty());
    assert!(result.reasoner_prompts.is_empty());
    let result = run_fixture(&[OPEN, READ], Some(true), false, "Unused");
    assert!(matches!(result.status, RunStatus::Failed { .. }));
    assert_eq!(result.calls.len(), 2);
    assert!(result.reasoner_prompts.is_empty());
}

#[test]
fn browser_starter_rejects_missing_or_incomplete_open_result_before_reading() {
    for complete in [None, Some(false)] {
        let result = run_fixture(&[OPEN, READ], complete, true, "Unused");
        assert!(matches!(result.status, RunStatus::Failed { .. }));
        assert_eq!(result.calls.len(), 1);
        assert_eq!(result.coder_prompts.len(), 1);
        assert!(result.reasoner_prompts.is_empty());
    }
}

#[test]
fn browser_starter_rejects_read_prose_and_unauthorized_read_task_tools() {
    for read_response in ["The page is ready", LIST, OPEN] {
        let result = run_fixture(&[OPEN, read_response], Some(true), true, "Unused");
        assert!(matches!(result.status, RunStatus::Failed { .. }));
        assert_eq!(result.calls.len(), 1);
        assert_eq!(result.coder_prompts.len(), 2);
        assert!(result.reasoner_prompts.is_empty());
    }
}

#[test]
fn browser_starter_rejects_model_selected_wrong_tab_before_dispatch() {
    // Native v3 incorrectly selected another existing tab (137). V4 must fail
    // before reading it even if the model supplies a syntactically valid ID.
    let wrong_read = r#"<tool_call>{"server":"e2llm","tool":"read_page","args":{"browser":"fixture","tabId":137}}</tool_call>"#;
    let result = run_fixture(&[OPEN, wrong_read], Some(true), true, "Unused");
    assert!(matches!(result.status, RunStatus::Failed { .. }));
    assert_eq!(result.calls.len(), 1);
    assert!(result.reasoner_prompts.is_empty());
}

#[test]
fn browser_starter_denies_reasoner_tool_dispatch() {
    let result = run_fixture(&[OPEN, READ], Some(true), true, LIST);
    assert!(matches!(result.status, RunStatus::Failed { .. }));
    assert_eq!(result.calls.len(), 2);
    assert_eq!(result.reasoner_prompts.len(), 1);
}
