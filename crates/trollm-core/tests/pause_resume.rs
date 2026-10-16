//! Pause mid-foreach → resume completes identically (§4.4). The paused run is
//! dropped and reconstructed from the run dir alone (state.snapshot.json +
//! events.jsonl), which is also the crash-recovery path.

mod common;

use common::{event_types, read_events, ScriptGateway};
use serde_json::Value;
use std::path::{Path, PathBuf};
use trollm_core::{ExecutionPlan, Executor, RunDir, RunStatus, SlotMap, TaskRegistry};
use trollm_models::MockModel;

fn main_plan(max_items: u32) -> String {
    format!(
        r#"
schemaVersion: ledger.execution-plan.v1
id: batch
version: 1
sourceBaseline: {{ sha256: "{}" }}
input: {{ promptTemplate: "go" }}
limits: {{ maxSteps: 50, maxReasonerCalls: 0, maxTokensTotal: 100000, timeoutS: 600, artifactMaxMb: 5 }}
onLimit: abort
models: {{ coder: {{ ctx: 8192 }} }}
mcp: {{ servers: [], policies: {{}} }}
entryStep: setup
nodes:
  setup: {{ kind: task, taskRef: prep, onVerified: each }}
  each:
    kind: foreach
    items: {{ from: setup.outputs.items }}
    itemKey: id
    bindAs: item
    enumeration: snapshot
    ordering: source
    maxItems: {max_items}
    onLimit: {{ status: paused, reason: item-limit }}
    body: {{ flowRef: unit, inputs: {{ item: {{ from: item }} }} }}
    outputs: {{ results: {{ collect: body.outputs, indexBy: itemKey }} }}
    onEmpty: done
    onComplete: finish
  finish: {{ kind: task, taskRef: summarize, onVerified: done }}
  done: {{ kind: end, outcome: succeeded }}
"#,
        "0".repeat(64)
    )
}

fn unit_plan() -> String {
    format!(
        r#"
schemaVersion: ledger.execution-plan.v1
id: unit
version: 1
sourceBaseline: {{ sha256: "{}" }}
input: {{ promptTemplate: "" }}
limits: {{ maxSteps: 5, maxReasonerCalls: 0, maxTokensTotal: 10000, timeoutS: 60, artifactMaxMb: 1 }}
onLimit: abort
models: {{ coder: {{ ctx: 8192 }} }}
mcp: {{ servers: [], policies: {{}} }}
entryStep: work
nodes:
  work: {{ kind: task, taskRef: work-task, onVerified: end-ok }}
  end-ok: {{ kind: end, outcome: succeeded }}
"#,
        "0".repeat(64)
    )
}

const REGISTRY: &str = r#"
schemaVersion: ledger.task-registry.v1
tasks:
  prep:
    model: coder
    prompt: prep.md
    tools: [t.list]
    loop: { maxSteps: 2 }
    outputContract:
      outputs:
        items: { fromArtifact: t.list.latest, format: jsonl, type: list }
      requireArtifactComplete: [t.list.latest]
  work-task:
    model: coder
    prompt: work.md
    tools: []
    loop: { maxSteps: 1 }
    outputContract:
      outputs: { note: { from: model, type: prose } }
      allowNoop: true
  summarize:
    model: coder
    prompt: summarize.md
    tools: []
    loop: { maxSteps: 1 }
    outputContract:
      outputs: { report: { from: model, type: prose } }
      allowNoop: true
"#;

const ITEMS: &[&str] = &[
    r#"{"id": 1, "brief": "a"}"#,
    r#"{"id": 2, "brief": "b"}"#,
    r#"{"id": 3, "brief": "c"}"#,
];

fn write_prompts(base: &Path) {
    std::fs::write(base.join("prep.md"), "list {{ user_input }}").unwrap();
    std::fs::write(base.join("work.md"), "work {{ item.id }}").unwrap();
    std::fs::write(
        base.join("summarize.md"),
        "report {{ each.outputs.results }}",
    )
    .unwrap();
}

fn make_plan(max_items: u32) -> ExecutionPlan {
    let mut plan = ExecutionPlan::from_yaml_str(&main_plan(max_items)).unwrap();
    let unit = ExecutionPlan::from_yaml_str(&unit_plan()).unwrap();
    plan.subflows.insert(unit.id.clone(), unit);
    plan
}

fn make_gateway(artifacts_dir: PathBuf) -> ScriptGateway {
    let mut gw = ScriptGateway::new(artifacts_dir);
    gw.items = ITEMS
        .iter()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    gw.artifact_tools.insert("t.list".to_string());
    gw
}

fn results_of(exec: &Executor<MockModel, ScriptGateway>) -> Value {
    exec.state().stack[0].vars["each"]["outputs"]["results"].clone()
}

#[test]
fn pause_mid_foreach_then_resume_completes_identically() {
    // --- control: uninterrupted run with a high item limit ------------------
    let base_c = common::tmp_dir("pause-control");
    write_prompts(&base_c);
    let run_dir_c = RunDir::under(&base_c);
    run_dir_c.create_dirs().unwrap();
    let coder_c = MockModel::scripted([
        r#"<tool_call>{"server":"t","tool":"list","args":{}}</tool_call>"#,
        "listed",
        "worked 1",
        "worked 2",
        "worked 3",
        "final report",
    ]);
    let mut exec_c = Executor::new(
        make_plan(500),
        TaskRegistry::from_yaml_str(REGISTRY, &base_c).unwrap(),
        "go".to_string(),
        SlotMap::new().with("coder", coder_c),
        make_gateway(run_dir_c.artifacts_dir()),
        run_dir_c.clone(),
    )
    .unwrap();
    assert_eq!(exec_c.run_until_blocked().unwrap(), RunStatus::Succeeded);
    let control_results = results_of(&exec_c);
    let control_types = event_types(&read_events(run_dir_c.path()));
    assert!(!control_types.iter().any(|t| t == "paused"));

    // --- paused run: maxItems 2 over a 3-item snapshot ----------------------
    let base_p = common::tmp_dir("pause-resume");
    write_prompts(&base_p);
    let run_dir_p = RunDir::under(&base_p);
    run_dir_p.create_dirs().unwrap();
    let coder_p1 = MockModel::scripted([
        r#"<tool_call>{"server":"t","tool":"list","args":{}}</tool_call>"#,
        "listed",
        "worked 1",
        "worked 2",
    ]);
    let mut exec_p1 = Executor::new(
        make_plan(2),
        TaskRegistry::from_yaml_str(REGISTRY, &base_p).unwrap(),
        "go".to_string(),
        SlotMap::new().with("coder", coder_p1),
        make_gateway(run_dir_p.artifacts_dir()),
        run_dir_p.clone(),
    )
    .unwrap();
    let status = exec_p1.run_until_blocked().unwrap();
    assert_eq!(
        status,
        RunStatus::Paused {
            reason: "item-limit".into()
        }
    );

    // state.snapshot.json holds the cursor at the pause boundary (§4.4)
    let snap: Value = serde_json::from_slice(
        &std::fs::read(run_dir_p.path().join("state.snapshot.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(snap["status"], "paused");
    assert_eq!(snap["foreachCursors"]["each"]["index"], 2);
    assert_eq!(
        snap["foreachCursors"]["each"]["results"]
            .as_array()
            .unwrap()
            .len(),
        2,
        "partial results persist across paused (§4.6)"
    );

    // Drop the executor entirely; resume from disk with fresh models/gateway.
    drop(exec_p1);
    let coder_p2 = MockModel::scripted(["worked 3", "final report"]);
    let mut exec_p2 = Executor::new(
        make_plan(2),
        TaskRegistry::from_yaml_str(REGISTRY, &base_p).unwrap(),
        "go".to_string(),
        SlotMap::new().with("coder", coder_p2),
        make_gateway(run_dir_p.artifacts_dir()),
        run_dir_p.clone(),
    )
    .unwrap();
    let status = exec_p2.resume().unwrap();
    assert_eq!(status, RunStatus::Succeeded);

    // identical aggregation to the uninterrupted run
    assert_eq!(results_of(&exec_p2), control_results);

    let types = event_types(&read_events(run_dir_p.path()));
    assert!(types.iter().any(|t| t == "paused"));
    assert!(types.iter().any(|t| t == "resumed"));
    assert_eq!(types.last().unwrap(), "run_ended");

    // same transition sequence as the control run, modulo the onLimit pause
    let keys_of = |dir: &Path| -> Vec<String> {
        read_events(dir)
            .iter()
            .filter(|e| e["type"] == "transition" && e["key"] != "onLimit")
            .map(|e| {
                format!(
                    "{}:{}:{}",
                    e["plan"].as_str().unwrap(),
                    e["from"].as_str().unwrap(),
                    e["key"].as_str().unwrap()
                )
            })
            .collect()
    };
    assert_eq!(keys_of(run_dir_p.path()), keys_of(run_dir_c.path()));
}
