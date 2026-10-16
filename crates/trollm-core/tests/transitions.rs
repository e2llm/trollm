//! Transition coverage (§4.2): statically checkable gaps are load-time
//! validation errors; the rest fail at runtime with `unhandled-transition`.

mod common;

use common::{read_events, ScriptGateway};
use trollm_core::{ExecutionPlan, Executor, RunDir, RunStatus, SlotMap, TaskRegistry};
use trollm_models::MockModel;

const EMPTY_REGISTRY: &str = "schemaVersion: ledger.task-registry.v1\ntasks: {}\n";

fn header(entry: &str) -> String {
    format!(
        r#"
schemaVersion: ledger.execution-plan.v1
id: t
version: 1
sourceBaseline: {{ sha256: "{}" }}
input: {{ promptTemplate: "go" }}
limits: {{ maxSteps: 10, maxReasonerCalls: 1, maxTokensTotal: 1000, timeoutS: 60, artifactMaxMb: 1 }}
onLimit: abort
models: {{ coder: {{ ctx: 8192 }} }}
mcp: {{ servers: [], policies: {{}} }}
entryStep: {entry}
"#,
        "0".repeat(64)
    )
}

fn try_build(plan_yaml: &str, registry_yaml: &str) -> anyhow::Result<()> {
    let base = common::tmp_dir("transitions");
    let plan = ExecutionPlan::from_yaml_str(plan_yaml)?;
    let registry = TaskRegistry::from_yaml_str(registry_yaml, &base)?;
    Executor::new(
        plan,
        registry,
        String::new(),
        SlotMap::new().with("coder", MockModel::default()),
        ScriptGateway::default(),
        RunDir::under(&base),
    )?;
    Ok(())
}

#[test]
fn unhandled_transition_at_runtime_fails_the_run() {
    // A root-level `end` with a subflow-only outcome ('skipped') is not
    // statically checkable (outcome is a free string in §3); at runtime it is
    // the §4.2 unhandled-transition failure.
    let base = common::tmp_dir("unhandled");
    let plan_yaml = format!(
        "{}\nnodes:\n  fin: {{ kind: end, outcome: skipped }}\n",
        header("fin")
    );
    let plan = ExecutionPlan::from_yaml_str(&plan_yaml).unwrap();
    let registry = TaskRegistry::from_yaml_str(EMPTY_REGISTRY, &base).unwrap();
    let mut exec = Executor::new(
        plan,
        registry,
        String::new(),
        SlotMap::new().with("coder", MockModel::default()),
        ScriptGateway::default(),
        RunDir::under(&base),
    )
    .unwrap();
    let status = exec.run_until_blocked().unwrap();
    match status {
        RunStatus::Failed { reason } => {
            assert!(reason.contains("unhandled-transition"), "{reason}")
        }
        other => panic!("expected Failed, got {other:?}"),
    }
}

#[test]
fn entry_step_must_exist() {
    let yaml = format!(
        "{}\nnodes:\n  done: {{ kind: end, outcome: succeeded }}\n",
        header("missing")
    );
    let err = try_build(&yaml, EMPTY_REGISTRY).unwrap_err();
    assert!(err.to_string().contains("entryStep"), "{err}");
}

#[test]
fn transition_targets_must_exist() {
    let yaml = format!(
        "{}\nnodes:\n  a: {{ kind: task, taskRef: x, onVerified: nowhere }}\n  done: {{ kind: end, outcome: succeeded }}\n",
        header("a")
    );
    let registry = "schemaVersion: ledger.task-registry.v1\ntasks:\n  x:\n    model: coder\n    prompt: p.md\n    tools: []\n    outputContract: { outputs: {}, allowNoop: true }\n";
    let err = try_build(&yaml, registry).unwrap_err();
    assert!(err.to_string().contains("nowhere"), "{err}");
}

#[test]
fn task_without_on_verified_is_a_static_error() {
    let yaml = format!(
        "{}\nnodes:\n  a: {{ kind: task, taskRef: x }}\n  done: {{ kind: end, outcome: succeeded }}\n",
        header("a")
    );
    let err = try_build(&yaml, EMPTY_REGISTRY).unwrap_err();
    assert!(err.to_string().contains("onVerified"), "{err}");
}

#[test]
fn foreach_requires_complete_and_empty_transitions() {
    let yaml = format!(
        r#"{}
nodes:
  each:
    kind: foreach
    items: {{ from: xs }}
    itemKey: id
    bindAs: item
    enumeration: snapshot
    ordering: source
    body: {{ flowRef: sub }}
    outputs: {{ results: {{ collect: body.outputs, indexBy: itemKey }} }}
    onComplete: done
  done: {{ kind: end, outcome: succeeded }}
"#,
        header("each")
    );
    let err = try_build(&yaml, EMPTY_REGISTRY).unwrap_err();
    assert!(err.to_string().contains("onEmpty"), "{err}");
}

#[test]
fn closed_enums_are_enforced() {
    // enumeration: only "snapshot" is legal (§3)
    let bad = format!(
        r#"{}
nodes:
  each:
    kind: foreach
    items: {{ from: xs }}
    itemKey: id
    bindAs: item
    enumeration: live
    ordering: source
    body: {{ flowRef: sub }}
    outputs: {{ results: {{ collect: body.outputs, indexBy: itemKey }} }}
    onEmpty: done
    onComplete: done
  done: {{ kind: end, outcome: succeeded }}
"#,
        header("each")
    );
    assert!(ExecutionPlan::from_yaml_str(&bad).is_err());

    // inline transition status: only paused|failed (§4.2)
    let bad2 = format!(
        "{}\nnodes:\n  a: {{ kind: task, taskRef: x, onVerified: done, onError: {{ retry: 0, then: {{ status: explode, reason: nope }} }} }}\n  done: {{ kind: end, outcome: succeeded }}\n",
        header("a")
    );
    assert!(ExecutionPlan::from_yaml_str(&bad2).is_err());

    // node kind is a closed set (§4.1)
    let bad3 = format!("{}\nnodes:\n  a: {{ kind: router }}\n", header("a"));
    assert!(ExecutionPlan::from_yaml_str(&bad3).is_err());
}

#[test]
fn task_ref_must_resolve_in_registry() {
    let yaml = format!(
        "{}\nnodes:\n  a: {{ kind: task, taskRef: ghost, onVerified: done }}\n  done: {{ kind: end, outcome: succeeded }}\n",
        header("a")
    );
    let err = try_build(&yaml, EMPTY_REGISTRY).unwrap_err();
    assert!(err.to_string().contains("ghost"), "{err}");
}

#[test]
fn model_slot_must_be_declared_and_bound() {
    // task uses slot 'reasoner' but the plan only declares 'coder'
    let yaml = format!(
        "{}\nnodes:\n  a: {{ kind: task, taskRef: x, onVerified: done }}\n  done: {{ kind: end, outcome: succeeded }}\n",
        header("a")
    );
    let registry = "schemaVersion: ledger.task-registry.v1\ntasks:\n  x:\n    model: reasoner\n    prompt: p.md\n    tools: []\n    outputContract: { outputs: {}, allowNoop: true }\n";
    let err = try_build(&yaml, registry).unwrap_err();
    assert!(err.to_string().contains("reasoner"), "{err}");
}

#[test]
fn events_log_seq_is_monotonic_on_failure() {
    let base = common::tmp_dir("seq");
    let plan_yaml = format!(
        "{}\nnodes:\n  fin: {{ kind: end, outcome: skipped }}\n",
        header("fin")
    );
    let run_dir = RunDir::under(&base);
    let mut exec = Executor::new(
        ExecutionPlan::from_yaml_str(&plan_yaml).unwrap(),
        TaskRegistry::from_yaml_str(EMPTY_REGISTRY, &base).unwrap(),
        String::new(),
        SlotMap::new().with("coder", MockModel::default()),
        ScriptGateway::default(),
        run_dir.clone(),
    )
    .unwrap();
    let _ = exec.run_until_blocked().unwrap();
    let events = read_events(run_dir.path());
    for (i, e) in events.iter().enumerate() {
        assert_eq!(e["seq"].as_u64().unwrap(), (i + 1) as u64);
    }
}
