//! Explicit one-call selection or abstention; scripted models, actual native lookup.
mod common;

use anyhow::Result;
use serde_json::json;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use trollm_core::artifacts::{self, ArtifactMeta};
use trollm_core::{ExecutionPlan, Executor, RunDir, RunStatus, SlotMap, TaskRegistry};
use trollm_mcp::{Provenance, ToolCall, ToolDescriptor, ToolGateway, ToolResult};
use trollm_models::MockModel;

const PLAN: &str = r#"
schemaVersion: ledger.execution-plan.v1
id: tool-or-route
version: 1
limits: { maxSteps: 8, maxReasonerCalls: 0, maxTokensTotal: 20000, timeoutS: 60, artifactMaxMb: 1 }
onLimit: abort
models: { coder: { ctx: 2048 } }
entryStep: work
nodes:
  work:
    kind: task
    taskRef: work
    onVerified: done
    routes: { more_evidence: done, unresolved: done }
    onError: { retry: 0, then: failed }
  done: { kind: end, outcome: succeeded }
  failed: { kind: end, outcome: failed }
"#;
const REGISTRY: &str = r#"
schemaVersion: ledger.task-registry.v1
tasks:
  work:
    model: coder
    prompt: work.md
    completion: tool-or-route
    tools: [local.select_unique, flow.choose]
    loop: { maxSteps: 1 }
    toolArguments:
      local.select_unique:
        modelFields: [equals]
        fromInput: { items: candidates }
        fixed: { field: /id, prefix: '' }
    outputContract:
      outputs:
        selection:
          fromArtifact: unique-selection
          format: json
          type: object
          schema:
            type: object
            required: [selected]
            properties:
              selected:
                type: object
                required: [id, kind]
                properties: { kind: { const: eligible } }
      requireArtifactComplete: [unique-selection]
"#;
const SELECT: &str = r#"<tool_call>{"server":"local","tool":"select_unique","args":{"equals":"observed"}}</tool_call>"#;
fn route(name: &str) -> String {
    format!(
        "<tool_call>{}</tool_call>",
        json!({"server":"flow","tool":"choose","args":{"route":name}})
    )
}
struct Gateway {
    run: RunDir,
    calls: Arc<Mutex<Vec<ToolCall>>>,
    write_artifact: bool,
    complete: bool,
    artifact_name: &'static str,
}
impl ToolGateway for Gateway {
    fn tool_class(&self, _: &str, _: &str) -> trollm_mcp::policy::ToolClass {
        trollm_mcp::policy::ToolClass::Read
    }
    fn list_tools(&self) -> Result<Vec<ToolDescriptor>> {
        Ok(trollm_mcp::local::descriptors())
    }
    fn validate_call(&self, _: &ToolCall) -> Result<()> {
        Ok(())
    }
    fn call(&mut self, call: &ToolCall) -> Result<ToolResult> {
        self.calls.lock().unwrap().push(call.clone());
        let output = trollm_mcp::local::invoke(&call.tool, &call.args)?;
        let mut paths = Vec::new();
        if self.write_artifact {
            let written = artifacts::write_atomic(
                &self.run.artifacts_dir(),
                &format!("{}.json", self.artifact_name),
                &serde_json::to_vec(&output)?,
            )?;
            artifacts::write_meta(
                &written.path,
                &ArtifactMeta {
                    fetched_at: 0,
                    complete: self.complete,
                },
            )?;
            paths.push(written.path);
        }
        Ok(ToolResult {
            content_for_model: output,
            artifacts: paths,
            provenance: Provenance::Local,
        })
    }
}
struct Fixture {
    base: PathBuf,
    run: RunDir,
    calls: Arc<Mutex<Vec<ToolCall>>>,
    artifact_name: &'static str,
}
impl Fixture {
    fn new() -> Self {
        let base = common::tmp_dir("tool-or-route");
        std::fs::write(
            base.join("work.md"),
            "Select an observed candidate, request more evidence, or finish unresolved.",
        )
        .unwrap();
        Self {
            run: RunDir::under(&base),
            base,
            calls: Arc::default(),
            artifact_name: "unique-selection",
        }
    }
    fn executor(
        &self,
        plan: &str,
        registry: &str,
        replies: &[String],
        write_artifact: bool,
        complete: bool,
    ) -> Result<Executor<MockModel, Gateway>> {
        Executor::new(ExecutionPlan::from_yaml_str(plan)?, TaskRegistry::from_yaml_str(registry, &self.base)?,
            json!({"candidates":[{"id":"observed","kind":"eligible","private":"original retained"},{"id":"other","kind":"other"}]}).to_string(),
            SlotMap::new().with("coder", MockModel::scripted(replies.iter().map(String::as_str))),
            Gateway { run: self.run.clone(), calls: self.calls.clone(), write_artifact, complete, artifact_name: self.artifact_name }, self.run.clone())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.base).unwrap();
    }
}

#[test]
fn bound_success_is_one_generation_and_preserves_original_selection() {
    let f = Fixture::new();
    let mut exec = f
        .executor(PLAN, REGISTRY, &[SELECT.into()], true, true)
        .unwrap();
    assert_eq!(exec.run_until_blocked().unwrap(), RunStatus::Succeeded);
    let vars = &exec.state().stack[0].vars;
    assert_eq!(
        vars["work"]["selection"],
        vars["work"]["outputs"]["selection"]
    );
    assert_eq!(
        vars["work"]["outputs"]["selection"]["selected"]["private"],
        "original retained"
    );
    let calls = f.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].args["field"], "/id");
    assert_eq!(calls[0].args["items"].as_array().unwrap().len(), 2);
    assert_eq!(
        vars["work"]["outputs"]["selection"]["sourceSha256"],
        trollm_mcp::CacheStore::hash_args(&calls[0].args["items"], None, None)
    );
    assert_eq!(
        common::read_events(f.run.path())
            .iter()
            .filter(|event| event["type"] == "model_call")
            .count(),
        1
    );
}

#[test]
fn each_declared_abstention_needs_no_artifact_and_publishes_no_success_outputs() {
    for name in ["more_evidence", "unresolved"] {
        let f = Fixture::new();
        let mut exec = f
            .executor(PLAN, REGISTRY, &[route(name)], false, false)
            .unwrap();
        assert_eq!(exec.run_until_blocked().unwrap(), RunStatus::Succeeded);
        assert!(f.calls.lock().unwrap().is_empty());
        assert!(exec.state().stack[0].vars.get("work").is_none());
        assert!(!exec.state().stack[0].node_outputs.contains_key("work"));
        assert!(common::read_events(f.run.path())
            .iter()
            .any(|event| event["key"] == format!("routes.{name}")));
    }
}

#[test]
fn revisit_decline_removes_prior_bare_and_outputs_aliases_but_keeps_original_file() {
    let f = Fixture::new();
    let plan = PLAN
        .replace("onVerified: done", "onVerified: again")
        .replace(
            "  done:",
            "  again: { kind: task, taskRef: again, routes: { revisit: work } }\n  done:",
        );
    let registry = format!("{REGISTRY}\n  again:\n    model: coder\n    prompt: work.md\n    completion: flow-route\n    tools: [flow.choose]\n");
    let mut exec = f
        .executor(
            &plan,
            &registry,
            &[SELECT.into(), route("revisit"), route("unresolved")],
            true,
            true,
        )
        .unwrap();
    assert_eq!(exec.run_until_blocked().unwrap(), RunStatus::Succeeded);
    assert!(exec.state().stack[0].vars.get("work").is_none());
    assert!(!exec.state().stack[0].node_outputs.contains_key("work"));
    assert!(f.run.artifacts_dir().join("unique-selection.json").exists());
    assert_eq!(f.calls.lock().unwrap().len(), 1);
}

#[test]
fn prose_mixed_calls_forged_arguments_and_undeclared_routes_fail_before_dispatch() {
    for reply in ["no target".into(), route("invented"), format!("{SELECT}{}", route("unresolved")),
        format!("{SELECT}{SELECT}"),
        r#"<tool_call>{"server":"local","tool":"select_unique","args":{"equals":"observed","items":[]}}</tool_call>"#.into(),
        r#"<tool_call>{"server":"flow","tool":"choose","args":{"route":"unresolved","reason":"x"}}</tool_call>"#.into()] {
        let f=Fixture::new(); let mut exec=f.executor(PLAN,REGISTRY,&[reply],true,true).unwrap();
        assert!(matches!(exec.run_until_blocked().unwrap(),RunStatus::Failed{..}));
        assert!(f.calls.lock().unwrap().is_empty());
    }
}

#[test]
fn ordinary_failure_missing_incomplete_or_invalid_outputs_never_becomes_abstention() {
    for (reply, write, complete) in [
        (SELECT.to_string(), false, true),
        (SELECT.to_string(), true, false),
        (SELECT.replace("observed", "missing"), true, true),
        (SELECT.replace("observed", "other"), true, true),
    ] {
        let f = Fixture::new();
        let mut exec = f
            .executor(PLAN, REGISTRY, &[reply], write, complete)
            .unwrap();
        assert!(matches!(
            exec.run_until_blocked().unwrap(),
            RunStatus::Failed { .. }
        ));
        assert!(exec.state().stack[0].vars.get("work").is_none());
        assert!(!common::read_events(f.run.path())
            .iter()
            .any(|event| event["from"] == "work"
                && event["key"]
                    .as_str()
                    .is_some_and(|key| key.starts_with("routes."))));
    }
    let f = Fixture::new();
    let old = artifacts::write_atomic(
        &f.run.artifacts_dir(),
        "unique-selection.json",
        br#"{"selected":{"id":"old","kind":"eligible"}}"#,
    )
    .unwrap();
    artifacts::write_meta(
        &old.path,
        &ArtifactMeta {
            fetched_at: 0,
            complete: true,
        },
    )
    .unwrap();
    let mut exec = f
        .executor(PLAN, REGISTRY, &[SELECT.into()], false, true)
        .unwrap();
    assert!(matches!(
        exec.run_until_blocked().unwrap(),
        RunStatus::Failed { .. }
    ));
    assert!(exec.state().stack[0].vars.get("work").is_none());
}

#[test]
fn registry_and_graph_reject_invalid_new_mode_without_relaxing_legacy_modes() {
    for registry in [
        REGISTRY.replace("model: coder", "model: other"),
        REGISTRY.replace("maxSteps: 1", "maxSteps: 2"),
        REGISTRY.replace(
            "tools: [local.select_unique, flow.choose]",
            "tools: [flow.choose]",
        ),
        REGISTRY.replace("      local.select_unique:", "      flow.choose:"),
        REGISTRY.replace(
            "      outputs:",
            "      outputs:\n        prose: { from: model, type: prose }",
        ),
        REGISTRY.replace(
            "    outputContract:",
            "    outputContract:\n      allowNoop: true",
        ),
    ] {
        let f = Fixture::new();
        assert!(TaskRegistry::from_yaml_str(&registry, &f.base).is_err());
    }
    for (plan,registry) in [(PLAN.replace("    onVerified: done\n",""),REGISTRY.to_string()),
        (PLAN.replace("    routes: { more_evidence: done, unresolved: done }\n",""),REGISTRY.to_string()),
        (PLAN.to_string(),REGISTRY.replace("completion: tool-or-route","completion: flow-route").replace("    toolArguments:\n      local.select_unique:\n        modelFields: [equals]\n        fromInput: { items: candidates }\n        fixed: { field: /id, prefix: '' }\n",""))] {
        let f=Fixture::new();assert!(f.executor(&plan,&registry,&[SELECT.into()],true,true).is_err());
    }
}

#[test]
fn effective_node_override_requires_only_replacement_and_keeps_original_schema() {
    let plan = PLAN.replace("    onVerified: done", "    outputs: { selection: { fromArtifact: replacement, format: json } }\n    onVerified: done");
    let registry = REGISTRY.replace("      requireArtifactComplete: [unique-selection]\n", "");
    for eligible in [true, false] {
        let mut f = Fixture::new();
        f.artifact_name = "replacement";
        let reply = if eligible {
            SELECT.to_string()
        } else {
            SELECT.replace("observed", "other")
        };
        let mut exec = f.executor(&plan, &registry, &[reply], true, true).unwrap();
        let status = exec.run_until_blocked().unwrap();
        if eligible {
            assert_eq!(status, RunStatus::Succeeded);
            assert_eq!(
                exec.state().stack[0].vars["work"]["outputs"]["selection"]["selected"]["id"],
                "observed"
            );
        } else {
            assert!(matches!(status, RunStatus::Failed { .. }));
            assert!(common::read_events(f.run.path())
                .iter()
                .any(|event| event["type"] == "error" && event["class"] == "verification-failed"));
        }
    }
    let mut f = Fixture::new();
    f.artifact_name = "replacement";
    let mut exec = f
        .executor(&plan, REGISTRY, &[SELECT.into()], true, true)
        .unwrap();
    assert!(matches!(
        exec.run_until_blocked().unwrap(),
        RunStatus::Failed { .. }
    ));
}

#[test]
fn decline_after_success_stays_output_free_through_durable_resume() {
    let f = Fixture::new();
    let plan = PLAN.replace("onVerified: done", "onVerified: again")
        .replace("unresolved: done", "unresolved: { status: paused, reason: declined, resumeAt: after }")
        .replace("  done:", "  again: { kind: task, taskRef: again, routes: { revisit: work } }\n  after: { kind: task, taskRef: after, onVerified: done }\n  done:");
    let registry = format!("{REGISTRY}\n  again: {{ model: coder, prompt: work.md, completion: flow-route, tools: [flow.choose] }}\n  after: {{ model: coder, prompt: work.md }}\n");
    let mut exec = f
        .executor(
            &plan,
            &registry,
            &[SELECT.into(), route("revisit"), route("unresolved")],
            true,
            true,
        )
        .unwrap();
    assert!(matches!(
        exec.run_until_blocked().unwrap(),
        RunStatus::Paused { .. }
    ));
    assert!(exec.state().stack[0].vars.get("work").is_none());
    drop(exec);
    let mut restored = f
        .executor(&plan, &registry, &["finished".into()], true, true)
        .unwrap();
    assert!(restored.state().stack[0].vars.get("work").is_none());
    assert!(!restored.state().stack[0].node_outputs.contains_key("work"));
    assert_eq!(restored.resume().unwrap(), RunStatus::Succeeded);
    assert!(restored.state().stack[0].vars.get("work").is_none());
    assert_eq!(f.calls.lock().unwrap().len(), 1);
}
