mod common;
use anyhow::Result;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use trollm_core::{
    artifacts::{self, ArtifactMeta},
    ExecutionPlan, Executor, RunDir, RunStatus, SlotMap, TaskRegistry,
};
use trollm_mcp::{
    policy::ToolClass, Provenance, ToolCall, ToolDescriptor, ToolGateway, ToolResult,
};
use trollm_models::MockModel;

const READ: &str = r#"<tool_call>{"server":"t","tool":"read","args":{}}</tool_call>"#;
const PAUSE: &str =
    r#"<tool_call>{"server":"flow","tool":"choose","args":{"route":"pause"}}</tool_call>"#;
const PLAN: &str = r#"
schemaVersion: ledger.execution-plan.v1
id: named-result
version: 1
limits: {maxSteps: 10, maxReasonerCalls: 0, maxTokensTotal: 20000, timeoutS: 60, artifactMaxMb: 1}
onLimit: abort
models: {coder: {ctx: 2048}}
mcp:
  servers: [{name: t, url: 'https://example.invalid/mcp'}]
  policies:
    t.read: {class: read, resultArtifact: browser-text}
entryStep: original
nodes:
  original: {kind: task, taskRef: original, onVerified: narrow, onError: {retry: 0, then: {status: failed, reason: failed}}}
  narrow: {kind: task, taskRef: narrow, onVerified: join, onError: {retry: 0, then: {status: failed, reason: failed}}}
  join: {kind: task, taskRef: join, onVerified: done}
  done: {kind: end, outcome: succeeded}
"#;
fn registry_value() -> Value {
    let make = |name: &str, scope: &str| json!({"model":"coder","prompt":"prompt.md","tools":["t.read"],"completion":"tool-result","resultArtifact":name,"loop":{"maxSteps":1},"toolArguments":{"t.read":{"modelFields":[],"fixed":{"scope":scope}}},"outputContract":{"outputs":{"page":{"fromArtifact":name,"format":"json","type":"object"}},"requireArtifactComplete":[name]}});
    json!({"schemaVersion":"ledger.task-registry.v1","tasks":{"original":make("selected-original","original"),"narrow":make("price-source","narrow"),"join":{"model":"coder","prompt":"prompt.md","tools":[],"outputContract":{"outputs":{"page":{"fromArtifact":"selected-original","format":"json","type":"object"}},"requireArtifactComplete":["selected-original"]}}}})
}
struct Gateway {
    dir: PathBuf,
    calls: Arc<Mutex<Vec<Value>>>,
    omit: bool,
}
impl ToolGateway for Gateway {
    fn list_tools(&self) -> Result<Vec<ToolDescriptor>> {
        Ok(vec![])
    }
    fn tool_class(&self, _: &str, _: &str) -> ToolClass {
        ToolClass::Read
    }
    fn validate_call(&self, _: &ToolCall) -> Result<()> {
        Ok(())
    }
    fn begin_result_artifact_attempt(&mut self, _: &str, _: &str, name: &str) -> Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        artifacts::write_meta(
            &self.dir.join(format!("{name}.json")),
            &ArtifactMeta {
                complete: false,
                fetched_at: 0,
            },
        )?;
        Ok(())
    }
    fn call(&mut self, c: &ToolCall) -> Result<ToolResult> {
        self.call_with_result_artifact(c, Some("browser-text"))
    }
    fn call_with_result_artifact(
        &mut self,
        c: &ToolCall,
        name: Option<&str>,
    ) -> Result<ToolResult> {
        self.calls.lock().unwrap().push(c.args.clone());
        let name = name.unwrap_or("browser-text");
        let value = json!({"content":c.args["scope"],"url":"https://example.invalid/observed"});
        let art = artifacts::write_atomic(
            &self.dir,
            &format!("{name}.json"),
            &serde_json::to_vec(&value)?,
        )?;
        artifacts::write_meta(
            &art.path,
            &ArtifactMeta {
                complete: true,
                fetched_at: 7,
            },
        )?;
        Ok(ToolResult {
            content_for_model: value,
            artifacts: if self.omit { vec![] } else { vec![art.path] },
            provenance: Provenance::Live,
        })
    }
}
#[test]
fn existing_join_retains_original_without_extra_model_call() {
    let base = common::tmp_dir("named-result");
    std::fs::write(base.join("prompt.md"), "Perform this task.").unwrap();
    let run = RunDir::under(&base);
    let calls = Arc::new(Mutex::new(vec![]));
    let mut exec = Executor::new(
        ExecutionPlan::from_yaml_str(PLAN).unwrap(),
        TaskRegistry::from_yaml_str(&registry_value().to_string(), &base).unwrap(),
        String::new(),
        SlotMap::new().with("coder", MockModel::scripted([READ, READ, "preserved"])),
        Gateway {
            dir: run.artifacts_dir(),
            calls: calls.clone(),
            omit: false,
        },
        run,
    )
    .unwrap();
    assert_eq!(exec.run_until_blocked().unwrap(), RunStatus::Succeeded);
    assert_eq!(
        exec.state().stack[0].vars["join"]["outputs"]["page"]["content"],
        "original"
    );
    assert_eq!(
        exec.state().stack[0].vars["narrow"]["outputs"]["page"]["content"],
        "narrow"
    );
    assert_eq!(exec.state().counters.steps, 3);
    assert_eq!(
        *calls.lock().unwrap(),
        vec![json!({"scope":"original"}), json!({"scope":"narrow"})]
    );
}
#[test]
fn current_call_artifact_is_required_even_if_complete_file_exists() {
    let base = common::tmp_dir("named-missing");
    std::fs::write(base.join("prompt.md"), "Read.").unwrap();
    let run = RunDir::under(&base);
    let mut exec = Executor::new(
        ExecutionPlan::from_yaml_str(PLAN).unwrap(),
        TaskRegistry::from_yaml_str(&registry_value().to_string(), &base).unwrap(),
        String::new(),
        SlotMap::new().with("coder", MockModel::scripted([READ])),
        Gateway {
            dir: run.artifacts_dir(),
            calls: Arc::new(Mutex::new(vec![])),
            omit: true,
        },
        run,
    )
    .unwrap();
    assert!(matches!(
        exec.run_until_blocked().unwrap(),
        RunStatus::Failed { .. }
    ));
    assert!(exec.state().stack[0].vars.get("original").is_none());
}
#[test]
fn malformed_revisit_clears_old_success_and_completeness_on_warm_and_cold_resume() {
    for cold in [false, true] {
        let base = common::tmp_dir("named-resume");
        std::fs::write(base.join("prompt.md"), "One task.").unwrap();
        let run = RunDir::under(&base);
        let mut plan = ExecutionPlan::from_yaml_str(PLAN).unwrap();
        plan.nodes.get_mut("original").unwrap().on_verified =
            Some(trollm_core::plan::Transition::Node("pause".into()));
        let pause: trollm_core::plan::Node=serde_yaml::from_str("kind: task\ntaskRef: pause\nroutes: {pause: {status: paused, reason: test, resumeAt: original}}").unwrap();
        plan.nodes.insert("pause".into(), pause);
        let mut value = registry_value();
        value["tasks"]["pause"] = json!({"model":"coder","prompt":"prompt.md","tools":["flow.choose"],"completion":"flow-route"});
        let registry = TaskRegistry::from_yaml_str(&value.to_string(), &base).unwrap();
        let create = |model| {
            Executor::new(
                plan.clone(),
                registry.clone(),
                String::new(),
                SlotMap::new().with("coder", model),
                Gateway {
                    dir: run.artifacts_dir(),
                    calls: Arc::new(Mutex::new(vec![])),
                    omit: false,
                },
                run.clone(),
            )
            .unwrap()
        };
        let mut exec = create(MockModel::scripted([READ, PAUSE, "not a tool"]));
        assert!(matches!(
            exec.run_until_blocked().unwrap(),
            RunStatus::Paused { .. }
        ));
        assert!(exec.state().stack[0].vars.get("original").is_some());
        if cold {
            drop(exec);
            exec = create(MockModel::scripted(["not a tool"]));
        }
        assert!(matches!(exec.resume().unwrap(), RunStatus::Failed { .. }));
        assert!(exec.state().stack[0].vars.get("original").is_none());
        assert!(!artifacts::is_complete(
            &run.artifacts_dir().join("selected-original.json")
        ));
    }
}
#[test]
fn invalidation_failure_cannot_route_to_a_stale_join_or_resume_it() {
    let base = common::tmp_dir("named-invalidation-failure");
    std::fs::write(base.join("prompt.md"), "Read.").unwrap();
    let run = RunDir::under(&base);
    let old = artifacts::write_atomic(
        &run.artifacts_dir(),
        "selected-original.json",
        br#"{"content":"stale"}"#,
    )
    .unwrap();
    artifacts::write_meta(
        &old.path,
        &ArtifactMeta {
            complete: true,
            fetched_at: 7,
        },
    )
    .unwrap();
    // A deterministic metadata write failure that leaves the old valid
    // sidecar, artifact and run journal intact, including when running as root.
    std::fs::create_dir(artifacts::meta_path(&old.path).with_extension("tmp")).unwrap();
    let plan = ExecutionPlan::from_yaml_str(&PLAN.replace(
        "original: {kind: task, taskRef: original, onVerified: narrow, onError: {retry: 0, then: {status: failed, reason: failed}}}",
        "original: {kind: task, taskRef: original, onVerified: narrow, onError: {retry: 2, then: join}}",
    ))
    .unwrap();
    let registry = TaskRegistry::from_yaml_str(&registry_value().to_string(), &base).unwrap();
    let calls = Arc::new(Mutex::new(vec![]));
    let create = || {
        Executor::new(
            plan.clone(),
            registry.clone(),
            String::new(),
            // Before the fix, onError reached the join, which accepted the
            // still-complete old artifact with this ordinary model response.
            SlotMap::new().with("coder", MockModel::scripted(["accepted stale result"])),
            Gateway {
                dir: run.artifacts_dir(),
                calls: calls.clone(),
                omit: false,
            },
            run.clone(),
        )
        .unwrap()
    };
    let mut exec = create();
    let status = exec.run_until_blocked().unwrap();
    assert!(
        matches!(&status, RunStatus::Failed { reason } if reason.contains("resultArtifact invalidation")),
        "unexpected outcome after failed invalidation: {status:?}"
    );
    assert_eq!(exec.state().counters.steps, 0);
    assert!(exec.state().stack[0].vars.get("join").is_none());
    assert!(calls.lock().unwrap().is_empty());
    assert!(artifacts::is_complete(&old.path));
    drop(exec);
    let mut reloaded = create();
    assert_eq!(reloaded.resume().unwrap(), status);
    assert_eq!(reloaded.state().counters.steps, 0);
    assert!(reloaded.state().stack[0].vars.get("join").is_none());
}

#[test]
fn registry_rejects_invalid_shape_and_policy_or_output_mismatch() {
    for value in [
        Value::Null,
        json!("../bad"),
        json!("calculation"),
        json!("model-failure-any"),
    ] {
        let mut r = registry_value();
        r["tasks"]["original"]["resultArtifact"] = value;
        assert!(TaskRegistry::from_yaml_str(&r.to_string(), ".").is_err());
    }
    for (key, value) in [
        ("completion", json!("model-prose")),
        ("tools", json!(["local.calculate"])),
        ("tools", json!(["t.read", "t.other"])),
        ("loop", json!({"maxSteps":2})),
    ] {
        let mut r = registry_value();
        r["tasks"]["original"][key] = value;
        assert!(TaskRegistry::from_yaml_str(&r.to_string(), ".").is_err());
    }
    let base = common::tmp_dir("named-validation");
    std::fs::write(base.join("prompt.md"), "Read.").unwrap();
    let r = TaskRegistry::from_yaml_str(&registry_value().to_string(), &base).unwrap();
    for plan in [PLAN.replace("class: read","class: mutation"),PLAN.replace("resultArtifact: browser-text","cache: {key: [], ttlS: 60}"),PLAN.replace("resultArtifact: browser-text","resultArtifact: browser-text, paginate: {param: cursor, deliver: first_page, backgroundFetch: all}")] {
        assert!(trollm_core::validate::validate_with_registry(&ExecutionPlan::from_yaml_str(&plan).unwrap(),&r).is_err());
    }
    let mut r = registry_value();
    r["tasks"]["original"]["outputContract"]["outputs"]["page"]["fromArtifact"] =
        json!("browser-text");
    let r = TaskRegistry::from_yaml_str(&r.to_string(), &base).unwrap();
    assert!(trollm_core::validate::validate_with_registry(
        &ExecutionPlan::from_yaml_str(PLAN).unwrap(),
        &r
    )
    .is_err());
}
struct Unsupported(Arc<Mutex<usize>>);
impl ToolGateway for Unsupported {
    fn list_tools(&self) -> Result<Vec<ToolDescriptor>> {
        Ok(vec![])
    }
    fn tool_class(&self, _: &str, _: &str) -> ToolClass {
        ToolClass::Read
    }
    fn validate_call(&self, _: &ToolCall) -> Result<()> {
        Ok(())
    }
    fn call(&mut self, _: &ToolCall) -> Result<ToolResult> {
        *self.0.lock().unwrap() += 1;
        Ok(ToolResult {
            content_for_model: json!({}),
            artifacts: vec![],
            provenance: Provenance::Live,
        })
    }
}
#[test]
fn default_gateway_forwards_legacy_call_but_rejects_override_before_generation() {
    let calls = Arc::new(Mutex::new(0));
    let mut gateway = Unsupported(calls.clone());
    let call = ToolCall {
        server: "t".into(),
        tool: "read".into(),
        args: json!({}),
    };
    gateway.call_with_result_artifact(&call, None).unwrap();
    assert!(gateway
        .call_with_result_artifact(&call, Some("selected-original"))
        .is_err());
    assert_eq!(*calls.lock().unwrap(), 1);
    let base = common::tmp_dir("named-unsupported");
    std::fs::write(base.join("prompt.md"), "Read.").unwrap();
    let run = RunDir::under(&base);
    let old =
        artifacts::write_atomic(&run.artifacts_dir(), "selected-original.json", b"{}").unwrap();
    artifacts::write_meta(
        &old.path,
        &ArtifactMeta {
            complete: true,
            fetched_at: 7,
        },
    )
    .unwrap();
    let mut exec = Executor::new(
        ExecutionPlan::from_yaml_str(PLAN).unwrap(),
        TaskRegistry::from_yaml_str(&registry_value().to_string(), &base).unwrap(),
        String::new(),
        SlotMap::new().with("coder", MockModel::scripted([READ])),
        gateway,
        run,
    )
    .unwrap();
    assert!(matches!(
        exec.run_until_blocked().unwrap(),
        RunStatus::Failed { .. }
    ));
    assert_eq!(exec.state().counters.steps, 0);
    assert_eq!(*calls.lock().unwrap(), 1);
    assert!(!artifacts::is_complete(&old.path));
}
