//! Closed authoring objects reject typos; dynamic names remain available.
//! These are parser and scripted-construction checks, not live acceptance.
mod common;

use serde_json::{json, Value};
use trollm_core::{ExecutionPlan, Executor, RunDir, SlotMap, TaskRegistry};

fn plan_value() -> Value {
    json!({
        "schemaVersion": "ledger.execution-plan.v1", "id": "integrity", "version": 1,
        "limits": {"maxSteps": 4, "maxReasonerCalls": 1, "maxTokensTotal": 100,
                   "timeoutS": 60, "artifactMaxMb": 1},
        "onLimit": "abort", "models": {"coder": {"ctx": 2048}},
        "entryStep": "work", "nodes": {
            "work": {"kind": "task", "taskRef": "work", "onVerified": "done"},
            "done": {"kind": "end", "outcome": "succeeded"}
        }
    })
}

fn registry_value() -> Value {
    json!({"schemaVersion": "ledger.task-registry.v1", "tasks": {
        "work": {"model": "coder", "prompt": "work.md", "tools": []}
    }})
}

#[test]
fn plan_boundaries_reject_unknown_keys_with_context() {
    for (path, extra) in [
        ("", json!({"unrecognized": true})),
        ("/input", json!({"promptTemplat": "typo"})),
        ("/limits", json!({"maxStep": 4})),
        ("/models/coder", json!({"context": 2048})),
        ("/mcp", json!({"server": []})),
        ("/nodes/work", json!({"onEror": {"retry": 3}})),
        (
            "/nodes/work/onError",
            json!({"retry": 0, "then": "done", "backoff": 1}),
        ),
        ("/nodes/work/verify", json!({"artifactComplet": "page"})),
    ] {
        let mut value = plan_value();
        if !path.is_empty() && value.pointer(path).is_none() {
            // All absent test objects are direct children except node fields.
            let (parent, key) = path.rsplit_once('/').unwrap();
            value.pointer_mut(parent).unwrap()[key] = json!({});
        }
        value
            .pointer_mut(path)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let error = ExecutionPlan::from_yaml_str(&value.to_string()).unwrap_err();
        assert!(
            error.to_string().contains("unknown field"),
            "{path}: {error}"
        );
    }
}

#[test]
fn nested_output_and_transition_objects_are_closed() {
    for (field, value) in [
        ("onVerified", json!({"status": "paused", "reasn": "typo"})),
        (
            "outputs",
            json!({"dynamic_output": {"from": "model", "typo": true}}),
        ),
        (
            "outputs",
            json!({"dynamic_output": {"fromArtifact": "page", "typo": true}}),
        ),
        ("items", json!({"from": "items", "typo": true})),
        ("body", json!({"flowRef": "child", "typo": true})),
        (
            "body",
            json!({"flowRef": "child", "inputs": {"dynamic": {"from": "item", "typo": true}}}),
        ),
        (
            "outputs",
            json!({"dynamic_output": {"collect": "body.outputs", "indexBy": "id", "typo": true}}),
        ),
    ] {
        let mut plan = plan_value();
        plan["nodes"]["work"][field] = value;
        assert!(
            ExecutionPlan::from_yaml_str(&plan.to_string()).is_err(),
            "{plan}"
        );
    }
}

#[test]
fn registry_boundaries_reject_unknown_keys() {
    for (path, extra) in [
        ("", json!({"typo": true})),
        (
            "/tasks/work",
            json!({"mutation": {"idempotency": "intent-log"}}),
        ),
        ("/tasks/work/loop", json!({"maxSteps": 1, "typo": true})),
        (
            "/tasks/work/mutations",
            json!({"idempotency": "intent-log", "onUncertain": "fail-run", "typo": true}),
        ),
        ("/tasks/work/outputContract", json!({"allowNoo": true})),
        (
            "/tasks/work/outputContract",
            json!({"outputs": {"dynamic": {"from": "model", "type": "prose", "typo": true}}}),
        ),
        (
            "/tasks/work/toolArguments",
            json!({"tracker.read": {"fromVars": {"id": "item.id"}, "typo": true}}),
        ),
    ] {
        let mut value = registry_value();
        if value.pointer(path).is_none() {
            let (parent, key) = path.rsplit_once('/').unwrap();
            value.pointer_mut(parent).unwrap()[key] = json!({});
        }
        value
            .pointer_mut(path)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        assert!(
            TaskRegistry::from_yaml_str(&value.to_string(), ".").is_err(),
            "{value}"
        );
    }
}

#[test]
fn canonical_reasoner_grants_reject_in_registry_and_executor_paths() {
    let mut value = registry_value();
    value["tasks"]["work"]["model"] = json!("reasoner");
    value["tasks"]["work"]["tools"] = json!(["tracker.read"]);
    let error = TaskRegistry::from_yaml_str(&value.to_string(), ".").unwrap_err();
    assert!(error.to_string().contains("task 'work'"), "{error}");
    assert!(error.to_string().contains("reasoner"), "{error}");

    // Programmatic construction cannot bypass the registry loader's guard.
    let registry: TaskRegistry = serde_json::from_value(value).unwrap();
    let plan = ExecutionPlan::from_yaml_str(&plan_value().to_string()).unwrap();
    let base = common::tmp_dir("reasoner-grant-integrity");
    let run = RunDir::under(&base);
    let error = Executor::new(
        plan,
        registry,
        String::new(),
        SlotMap::<trollm_models::MockModel>::new(),
        common::ScriptGateway::new(run.artifacts_dir()),
        run,
    )
    .err()
    .unwrap();
    assert!(error.to_string().contains("reasoner"), "{error}");
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn dynamic_names_and_noncanonical_slots_remain_supported() {
    let mut plan = plan_value();
    plan["models"] = json!({"custom_slot": {"ctx": 2048}});
    plan["nodes"]["work"]["outputs"] =
        json!({"custom_output": {"fromArtifact": "page", "format": "json"}});
    let parsed = ExecutionPlan::from_yaml_str(&plan.to_string()).unwrap();
    trollm_core::validate::validate_plan(&parsed).unwrap();
    let mut registry = registry_value();
    registry["tasks"]["work"] = json!({"model": "custom_slot", "prompt": "work.md",
        "tools": ["custom_server.custom_tool"], "completion": "tool-result",
        "toolArguments": {"custom_server.custom_tool": {"fromVars": {"custom_argument": "item.id"}}},
        "outputContract": {"outputs": {"custom_output": {"fromArtifact": "page", "format": "json", "type": "object"}}}
    });
    TaskRegistry::from_yaml_str(&registry.to_string(), ".").unwrap();
    registry["tasks"]["work"] = json!({"model": "reasoner", "prompt": "work.md", "tools": []});
    TaskRegistry::from_yaml_str(&registry.to_string(), ".").unwrap();
}

#[test]
fn opaque_plan_policy_typos_reject_during_validation() {
    let mut value = plan_value();
    value["mcp"] = json!({"policies": {"dynamic_server.dynamic_tool": {"rety": {"attempts": 2}}}});
    let plan = ExecutionPlan::from_yaml_str(&value.to_string()).unwrap();
    let error = trollm_core::validate::validate_plan(&plan).unwrap_err();
    assert!(error.to_string().contains("rety"), "{error}");
}

#[test]
fn provenance_is_strict_only_at_explicit_new_authoring_boundary() {
    let registry = TaskRegistry::from_yaml_str(&registry_value().to_string(), ".").unwrap();
    for source in [
        "",
        "deadbeef",
        &"A".repeat(64),
        &"g".repeat(64),
        &"a".repeat(63),
        &"a".repeat(65),
    ] {
        let mut value = plan_value();
        value["sourceBaseline"] = json!({"sha256": source});
        let plan = ExecutionPlan::from_yaml_str(&value.to_string()).unwrap();
        trollm_core::validate::validate_plan(&plan).unwrap();
        let error =
            trollm_core::validate::validate_authoring_provenance(&plan, &registry).unwrap_err();
        assert!(
            error.to_string().contains("sourceBaseline.sha256"),
            "{error}"
        );
    }
    let mut plan = ExecutionPlan::from_yaml_str(&plan_value().to_string()).unwrap();
    trollm_core::validate::validate_authoring_provenance(&plan, &registry).unwrap();
    plan.source_baseline = Some(trollm_core::plan::SourceBaseline {
        sha256: "0".repeat(64),
    });
    trollm_core::validate::validate_authoring_provenance(&plan, &registry).unwrap();
    let mut child = plan.clone();
    child.id = "child".into();
    child.source_baseline.as_mut().unwrap().sha256 = "deadbeef".into();
    plan.subflows.insert("child".into(), child);
    let error = trollm_core::validate::validate_authoring_provenance(&plan, &registry).unwrap_err();
    assert!(error.to_string().contains("plan 'child'"), "{error}");
    plan.subflows.clear();
    let mut registry = registry;
    registry.source_baseline = Some(trollm_core::plan::SourceBaseline {
        sha256: "deadbeef".into(),
    });
    assert!(trollm_core::validate::validate_authoring_provenance(&plan, &registry).is_err());
}

#[test]
fn registry_provenance_present_shape_matches_plan() {
    for source in [
        json!(null),
        json!({}),
        json!({"sha256": null}),
        json!({"sha256": 123}),
        json!({"sha256": "a", "typo": true}),
    ] {
        let mut registry = registry_value();
        registry["sourceBaseline"] = source;
        assert!(TaskRegistry::from_yaml_str(&registry.to_string(), ".").is_err());
    }
}
