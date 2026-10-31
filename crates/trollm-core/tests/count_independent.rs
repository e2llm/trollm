//! One unchanged plan with prepared input, reusable children and real local tools.
//! Models and external mutation/observation are scripted; no network or inference.
mod common;
use anyhow::Result;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use trollm_core::artifacts::{self, ArtifactMeta};
use trollm_core::{ExecutionPlan, Executor, RunDir, RunStatus, SlotMap, TaskRegistry};
use trollm_mcp::{
    McpGateway, PolicySet, Provenance, ToolCall, ToolDescriptor, ToolGateway, ToolResult,
};
use trollm_models::MockModel;

const ROOT: &str = r#"
schemaVersion: ledger.execution-plan.v1
id: count-independent
version: 1
limits: {maxSteps: 8, maxReasonerCalls: 0, maxTokensTotal: 10000, timeoutS: 60, artifactMaxMb: 4}
onLimit: abort
models: {coder: {ctx: 4096}}
mcp:
  policies:
    local.prepare_worklist: {resultArtifact: prepared}
    local.calculate: {resultArtifact: calculation}
    local.collection_total: {resultArtifact: total}
    mock.write: {class: mutation}
entryStep: prepare
nodes:
  prepare: {kind: task, taskRef: prepare, onVerified: discover}
  discover:
    kind: foreach
    items: {from: prepare.outputs.worklist.items}
    itemKey: id
    bindAs: item
    enumeration: snapshot
    ordering: source
    body: {flowRef: discovery, inputs: {request: {from: item}}}
    outputs:
      full: {collect: body.outputs, indexBy: itemKey}
      compact: {collect: body.outputs, indexBy: itemKey, view: {fields: {id: true, outcome: true, outputs: {amount: true}}, maxBytes: 16384}}
    onEmpty: {status: failed, reason: empty worklist}
    onComplete: total
  total: {kind: task, taskRef: total, onVerified: execute}
  execute:
    kind: foreach
    items: {from: prepare.outputs.worklist.items}
    itemKey: id
    bindAs: item
    enumeration: snapshot
    ordering: source
    body: {flowRef: mutation, inputs: {request: {from: item}, total: {from: total.outputs.total.sum}}}
    outputs: {results: {collect: body.outputs, indexBy: itemKey}}
    onEmpty: {status: failed, reason: empty worklist}
    onComplete: observe
  observe: {kind: task, taskRef: observe, onVerified: done}
  done: {kind: end, outcome: succeeded}
"#;
const CHILD: &str = r#"
schemaVersion: ledger.execution-plan.v1
id: discovery
version: 1
limits: {maxSteps: 2, maxReasonerCalls: 0, maxTokensTotal: 4096, timeoutS: 30, artifactMaxMb: 1}
onLimit: abort
models: {coder: {ctx: 4096}}
exports:
  request: {from: request}
  amount: {from: calculate.outputs.calculation.result}
entryStep: calculate
nodes:
  calculate: {kind: task, taskRef: calculate, onVerified: done}
  done: {kind: end, outcome: succeeded}
"#;
const MUTATION: &str = r#"
schemaVersion: ledger.execution-plan.v1
id: mutation
version: 1
limits: {maxSteps: 2, maxReasonerCalls: 0, maxTokensTotal: 4096, timeoutS: 30, artifactMaxMb: 1}
onLimit: abort
models: {coder: {ctx: 4096}}
exports: {request: {from: request}}
entryStep: write
nodes:
  write: {kind: task, taskRef: write, onVerified: done}
  done: {kind: end, outcome: succeeded}
"#;
const REGISTRY: &str = r#"
schemaVersion: ledger.task-registry.v1
tasks:
  prepare:
    model: coder
    prompt: task.md
    tools: [local.prepare_worklist]
    completion: tool-result
    toolArguments:
      local.prepare_worklist: {modelFields: [], fromInput: {input: prepared}}
    outputContract:
      outputs: {worklist: {fromArtifact: prepared, format: json, type: object}}
  calculate:
    model: coder
    prompt: task.md
    tools: [local.calculate]
    completion: tool-result
    toolArguments:
      local.calculate:
        modelFields: []
        fromVars: {left: request.value.amount}
        fixed: {operation: multiply, right: "1"}
    outputContract:
      outputs: {calculation: {fromArtifact: calculation, format: json, type: object}}
  total:
    model: coder
    prompt: task.md
    tools: [local.collection_total]
    completion: tool-result
    toolArguments:
      local.collection_total:
        modelFields: []
        fromVars: {expected: prepare.outputs.worklist.items, results: discover.outputs.compact}
        fixed: {expectedId: /id, resultId: /id, outcome: /outcome, amount: /outputs/amount}
    outputContract:
      outputs: {total: {fromArtifact: total, format: json, type: object}}
  write:
    model: coder
    prompt: task.md
    tools: [mock.write]
    completion: tool-result
    mutations: {idempotency: intent-log, onUncertain: fail-run}
    toolArguments:
      mock.write: {modelFields: [], fromVars: {request: request, total: total}}
  observe:
    model: coder
    prompt: task.md
    tools: [mock.observe]
    completion: tool-result
    outputContract:
      outputs: {observation: {fromArtifact: observed, format: json, type: object}}
"#;

struct Gateway {
    local: McpGateway,
    run: RunDir,
    writes: Arc<Mutex<Vec<Value>>>,
}
impl ToolGateway for Gateway {
    fn tool_class(&self, server: &str, tool: &str) -> trollm_mcp::policy::ToolClass {
        if server == "mock" && tool == "write" {
            trollm_mcp::policy::ToolClass::Mutation
        } else {
            trollm_mcp::policy::ToolClass::Read
        }
    }
    fn list_tools(&self) -> Result<Vec<ToolDescriptor>> {
        self.local.list_tools()
    }
    fn validate_call(&self, call: &ToolCall) -> Result<()> {
        if call.server == "local" {
            self.local.validate_call(call)
        } else {
            Ok(())
        }
    }
    fn call(&mut self, call: &ToolCall) -> Result<ToolResult> {
        if call.server == "local" {
            return self.local.call(call);
        }
        if call.tool == "write" {
            assert!(common::read_events(self.run.path())
                .iter()
                .any(|e| e["type"] == "tool_call_intent"));
            self.writes.lock().unwrap().push(call.args.clone());
            return Ok(ToolResult {
                content_for_model: json!({"ok":true}),
                artifacts: vec![],
                provenance: Provenance::Live,
            });
        }
        let payload = json!({"written":self.writes.lock().unwrap().clone()});
        let written = artifacts::write_atomic(
            &self.run.artifacts_dir(),
            "observed.json",
            &serde_json::to_vec(&payload)?,
        )?;
        artifacts::write_meta(
            &written.path,
            &ArtifactMeta {
                fetched_at: 0,
                complete: true,
            },
        )?;
        Ok(ToolResult {
            content_for_model: payload,
            artifacts: vec![written.path],
            provenance: Provenance::Live,
        })
    }
}
fn call(server: &str, tool: &str, args: Value) -> String {
    format!(
        "<tool_call>{}</tool_call>",
        json!({"server":server,"tool":tool,"args":args})
    )
}

#[test]
fn same_plan_composes_one_two_three_and_max_items_and_rejects_bound_override() {
    for (count, override_bound) in [
        (1, false),
        (2, false),
        (3, false),
        (8, false),
        (3, true),
        (0, false),
        (9, false),
    ] {
        let base = common::tmp_dir("count-independent");
        std::fs::write(
            base.join("task.md"),
            "Invoke the granted tool with the bound original evidence.",
        )
        .unwrap();
        let mut plan = ExecutionPlan::from_yaml_str(ROOT).unwrap();
        plan.subflows.insert(
            "discovery".into(),
            ExecutionPlan::from_yaml_str(CHILD).unwrap(),
        );
        plan.subflows.insert(
            "mutation".into(),
            ExecutionPlan::from_yaml_str(MUTATION).unwrap(),
        );
        let registry = TaskRegistry::from_yaml_str(REGISTRY, &base).unwrap();
        let run = RunDir::under(&base);
        let policies =
            PolicySet::from_yaml_value(&serde_yaml::to_value(&plan.mcp.policies).unwrap()).unwrap();
        let local = McpGateway::new(
            vec![],
            policies,
            base.join("cache"),
            run.artifacts_dir(),
            None,
        )
        .unwrap();
        let writes = Arc::new(Mutex::new(vec![]));
        let mut script = vec![call("local", "prepare_worklist", json!({}))];
        script.extend((0..count).map(|_| call("local", "calculate", json!({}))));
        script.push(call(
            "local",
            "collection_total",
            if override_bound {
                json!({"results":[]})
            } else {
                json!({})
            },
        ));
        script.extend((0..count).map(|_| call("mock", "write", json!({}))));
        script.push(call("mock", "observe", json!({})));
        let prepared = json!({"context":"original global constraints", "items":(0..count).map(|i| json!({"request":format!("original request {i}"),"amount":"1.25"})).collect::<Vec<_>>()});
        let mut exec = Executor::new(
            plan,
            registry,
            json!({"prepared":prepared.to_string()}).to_string(),
            SlotMap::new().with("coder", MockModel::scripted(script)),
            Gateway {
                local,
                run: run.clone(),
                writes: writes.clone(),
            },
            run.clone(),
        )
        .unwrap();
        let status = exec.run_until_blocked().unwrap();
        if override_bound || count == 0 || count > 8 {
            assert!(matches!(status, RunStatus::Failed { .. }));
            assert!(
                writes.lock().unwrap().is_empty(),
                "preflight failure prevents every write"
            );
        } else {
            assert_eq!(status, RunStatus::Succeeded);
            let vars = &exec.state().stack[0].vars;
            assert_eq!(vars["total"]["outputs"]["total"]["count"], count);
            assert_eq!(
                vars["total"]["outputs"]["total"]["sum"],
                match count {
                    1 => "1.25",
                    2 => "2.5",
                    3 => "3.75",
                    8 => "10",
                    _ => unreachable!(),
                }
            );
            assert_eq!(writes.lock().unwrap().len(), count);
            assert_eq!(
                vars["observe"]["outputs"]["observation"]["written"]
                    .as_array()
                    .unwrap()
                    .len(),
                count
            );
            for (index, record) in vars["discover"]["outputs"]["full"]
                .as_array()
                .unwrap()
                .iter()
                .enumerate()
            {
                assert_eq!(
                    record["outputs"]["request"]["value"]["request"],
                    format!("original request {index}")
                );
                assert!(run
                    .path()
                    .join(record["evidence"]["path"].as_str().unwrap())
                    .is_file());
            }
            let bytes = std::fs::metadata(run.snapshot_path()).unwrap().len();
            assert!(bytes < 256 * 1024, "scripted snapshot {bytes} bytes");
        }
        drop(exec);
        std::fs::remove_dir_all(base).unwrap();
    }
}

#[test]
fn explicit_export_and_collection_view_syntax_rejects_unsupported_shapes() {
    for exports in [
        "null",
        "{}",
        "{result: {from: 'request..id'}}",
        "{result: {from: request, default: missing}}",
    ] {
        let yaml = CHILD.replace("exports:\n  request: {from: request}\n  amount: {from: calculate.outputs.calculation.result}", &format!("exports: {exports}"));
        let parsed = ExecutionPlan::from_yaml_str(&yaml);
        assert!(
            parsed
                .as_ref()
                .map(trollm_core::validate::validate_plan)
                .map_or(true, |valid| valid.is_err()),
            "{exports}"
        );
    }
    for view in [
        json!(null),
        json!({"fields":{"outputs":false},"maxBytes":32}),
        json!({"fields":{"outputs":true},"maxBytes":0}),
    ] {
        assert!(serde_json::from_value::<trollm_core::plan::ForeachCollect>(
            json!({"collect":"body.outputs","indexBy":"itemKey","view":view})
        )
        .is_err());
    }
}
