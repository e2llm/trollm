//! Conformance test (§11): drives flows/examples/ticket-triage.flow.yaml with
//! its flowRefs (process-item, return-to-list) end-to-end using MockModel
//! instances and a scripted in-test ToolGateway.

mod common;

use common::{event_types, read_events, transitions, ScriptGateway};
use serde_json::json;
use std::path::{Path, PathBuf};
use trollm_core::{ExecutionPlan, Executor, RunDir, RunStatus, SlotMap, TaskRegistry};
use trollm_models::MockModel;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn load_plan() -> ExecutionPlan {
    ExecutionPlan::from_yaml_file(&repo_root().join("flows/examples/ticket-triage.flow.yaml"))
        .expect("ticket-triage plan loads")
}

fn load_registry() -> TaskRegistry {
    TaskRegistry::from_yaml_file(&repo_root().join("flows/tasks/tickets.tasks.yaml"))
        .expect("tickets registry loads")
}

fn issues() -> Vec<serde_json::Value> {
    vec![
        json!({"id": 1, "brief": "login broken"}),
        json!({"id": 2, "brief": "typo in docs"}),
        json!({"id": 3, "brief": "flaky test"}),
    ]
}

/// Coder script, in exact generation order:
/// prepare-worklist (fence, prose), then per item:
/// enrich (fence, prose), act, checkpoint; finally finalize-batch.
/// Item 2's `act` is the scripted no-op (§4.3 allowNoop canonical case).
fn coder_script() -> MockModel {
    MockModel::scripted([
        // setup / prepare-worklist
        r#"<tool_call>{"server":"tracker","tool":"list_issues","args":{"query":"old"}}</tool_call>"#,
        "Started the listing for old tickets; first page looks right.",
        // item 1: enrich, act (close), checkpoint
        r#"<tool_call>{"server":"tracker","tool":"get_issue","args":{"id":1}}</tool_call>"#,
        "Ticket 1 reports a login failure, high severity, still open.",
        r#"<tool_call>{"server":"tracker","tool":"close_issue","args":{"id":1}}</tool_call>"#,
        "Closed ticket 1 as advised.",
        "1 of 3 done.",
        // item 2: enrich, act (NO-OP), checkpoint
        r#"<tool_call>{"server":"tracker","tool":"get_issue","args":{"id":2}}</tool_call>"#,
        "Ticket 2 is a cosmetic docs typo, low severity.",
        "The advice says to do nothing, so I made no changes.",
        "2 of 3 done.",
        // item 3: enrich, act (comment), checkpoint
        r#"<tool_call>{"server":"tracker","tool":"get_issue","args":{"id":3}}</tool_call>"#,
        "Ticket 3 is a flaky test report, medium severity.",
        r#"<tool_call>{"server":"tracker","tool":"add_comment","args":{"id":3,"body":"noted"}}</tool_call>"#,
        "Added a comment to ticket 3.",
        "3 of 3 done.",
        // finalize-batch
        "Processed 3 tickets: closed 1, skipped 1, commented 1.",
    ])
}

fn reasoner_script() -> MockModel {
    MockModel::scripted([
        "Close this ticket; it is a duplicate of the fixed login bug.",
        "Do nothing; a typo does not warrant a change.",
        "Add a comment noting the flake is being tracked.",
    ])
}

#[test]
fn ticket_triage_conformance() {
    let base = common::tmp_dir("conformance");
    let run_dir = RunDir::under(&base);
    run_dir.create_dirs().unwrap();

    let mut gateway = ScriptGateway::new(run_dir.artifacts_dir());
    gateway.items = issues();
    gateway
        .artifact_tools
        .insert("tracker.list_issues".to_string());
    gateway.record_tools.insert("tracker.get_issue".to_string());

    let models = SlotMap::new()
        .with("coder", coder_script())
        .with("reasoner", reasoner_script());

    let mut exec = Executor::new(
        load_plan(),
        load_registry(),
        "close the old tickets".to_string(),
        models,
        gateway,
        run_dir.clone(),
    )
    .expect("executor constructs (plan + registry validate)");

    let status = exec.run_until_blocked().expect("run completes");
    assert_eq!(status, RunStatus::Succeeded, "run must succeed");

    let events = read_events(run_dir.path());
    let types = event_types(&events);

    // --- event log: required types, monotonic seq, envelope order (§7) ------
    assert_eq!(types.first().unwrap(), "run_started");
    assert_eq!(types.last().unwrap(), "run_ended");
    for (i, e) in events.iter().enumerate() {
        assert_eq!(e["seq"].as_u64().unwrap(), (i + 1) as u64, "seq monotonic");
    }
    for required in [
        "run_started",
        "plan_loaded",
        "node_entered",
        "node_exited",
        "model_call",
        "tool_call_intent",
        "tool_call_result",
        "artifact_written",
        "transition",
        "run_ended",
    ] {
        assert!(
            types.iter().any(|t| t == required),
            "missing event type {required}"
        );
    }
    // run_ended carries the outcome.
    assert_eq!(events.last().unwrap()["outcome"], "succeeded");
    // artifact_written events record path, bytes, sha256 (§7).
    for e in events.iter().filter(|e| e["type"] == "artifact_written") {
        assert!(e["path"].is_string() && e["bytes"].is_u64() && e["sha256"].is_string());
    }
    // plan_loaded for all three flows, sha256-pinned (§4.4 frame identity).
    let loaded: Vec<&str> = events
        .iter()
        .filter(|e| e["type"] == "plan_loaded")
        .map(|e| e["plan"].as_str().unwrap())
        .collect();
    assert_eq!(loaded, ["ticket-triage", "process-item", "return-to-list"]);

    // --- transition sequence (§4.2 closed keys) ------------------------------
    let trs = transitions(&events);
    assert_eq!(
        trs[0],
        (
            "ticket-triage".into(),
            "setup".into(),
            "each".into(),
            "onVerified".into()
        )
    );
    let key = |k: &str| trs.iter().filter(|t| t.3 == k).count();
    assert_eq!(key("body"), 3, "one body frame per snapshot item");
    assert_eq!(key("between"), 3, "one between frame per item");
    assert_eq!(key("onComplete"), 1);
    // the subflow item path: enrich -> decide -> act -> end-item, per item
    let item_path: Vec<(String, String, String)> = trs
        .iter()
        .filter(|t| t.0 == "process-item")
        .map(|t| (t.1.clone(), t.2.clone(), t.3.clone()))
        .collect();
    assert_eq!(item_path.len(), 9, "3 item nodes x 3 items");
    assert_eq!(
        item_path[0..3],
        [
            ("enrich".into(), "decide".into(), "onVerified".into()),
            ("decide".into(), "act".into(), "onVerified".into()),
            ("act".into(), "end-item".into(), "onVerified".into()),
        ]
    );
    // tail of the run: foreach completes, finalize, done
    let tail: Vec<(String, String, String)> = trs[trs.len() - 2..]
        .iter()
        .map(|t| (t.1.clone(), t.2.clone(), t.3.clone()))
        .collect();
    assert_eq!(
        tail,
        [
            ("each".into(), "finish".into(), "onComplete".into()),
            ("finish".into(), "done".into(), "onVerified".into()),
        ]
    );

    // --- foreach iterated the snapshot; aggregation shape (§4.6) -------------
    let state = exec.state();
    let root = &state.stack[0];
    let results = root.vars["each"]["outputs"]["results"]
        .as_array()
        .expect("each.outputs.results is an array");
    assert_eq!(results.len(), 3, "foreach iterated the full snapshot");
    let ids: Vec<i64> = results.iter().map(|r| r["id"].as_i64().unwrap()).collect();
    assert_eq!(ids, [1, 2, 3], "source ordering preserved");
    for r in results {
        assert_eq!(r["outcome"], "succeeded");
        assert!(r["outputs"]["enrich"]["summary"].is_string());
        assert!(r["outputs"]["decide"]["advice"].is_string());
        assert!(r["outputs"]["act"]["outcome"].is_string());
    }
    // the no-op item recorded its prose outcome without a mutation
    assert!(results[1]["outputs"]["act"]["outcome"]
        .as_str()
        .unwrap()
        .contains("nothing"));

    // --- mutation journaling happened for the two writes (§6) ----------------
    let intents: Vec<&serde_json::Value> = events
        .iter()
        .filter(|e| e["type"] == "tool_call_intent")
        .collect();
    assert_eq!(intents.len(), 2, "close_issue + add_comment journaled");
    assert!(intents.iter().all(|e| e["argsSha256"].is_string()));
    let results_ev: Vec<&serde_json::Value> = events
        .iter()
        .filter(|e| e["type"] == "tool_call_result")
        .collect();
    // list_issues + 3x get_issue + 2 mutations = 6
    assert_eq!(results_ev.len(), 6);
    // every intent was answered
    for i in intents {
        let seq = i["seq"].as_u64().unwrap();
        assert!(results_ev
            .iter()
            .any(|r| r["intentSeq"].as_u64() == Some(seq)));
    }
    // the gateway saw exactly two mutations (the no-op made none)
    let mutation_calls = read_gateway_mutation_count(&events);
    assert_eq!(mutation_calls, 2);

    // --- state snapshot was persisted at the final boundary (§4.4) -----------
    let snap: serde_json::Value =
        serde_json::from_slice(&std::fs::read(run_dir.path().join("state.snapshot.json")).unwrap())
            .unwrap();
    assert_eq!(snap["status"], "succeeded");
    assert!(snap["counters"]["steps"].as_u64().unwrap() >= 15);
}

fn read_gateway_mutation_count(events: &[serde_json::Value]) -> usize {
    events
        .iter()
        .filter(|e| {
            e["type"] == "tool_call_result"
                && ["close_issue", "update_issue", "add_comment"]
                    .contains(&e["tool"].as_str().unwrap_or(""))
        })
        .count()
}
