//! Cooperative cancellation of scripted runs, including a configured delay.
mod common;

use common::{read_events, ScriptGateway};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use trollm_core::exec::ExecutionControl;
use trollm_core::{ExecutionPlan, Executor, RunDir, RunStatus, SlotMap, TaskRegistry};
use trollm_mcp::{ToolCall, ToolDescriptor, ToolGateway, ToolResult};
use trollm_models::{FinishReason, GenerateRequest, GenerateResponse, ModelSlot};

const FENCE: &str = r#"<tool_call>{"server":"t","tool":"flaky","args":{"x":1}}</tool_call>"#;
const REGISTRY: &str = r#"
schemaVersion: ledger.task-registry.v1
tasks:
  flaky-task:
    model: coder
    prompt: prompt.md
    tools: [t.flaky]
    loop: { maxSteps: 4 }
    outputContract:
      outputs: { outcome: { from: model, type: prose } }
"#;

fn plan(delay: u64, mutation: bool) -> ExecutionPlan {
    let retry = if mutation { 0 } else { 2 };
    ExecutionPlan::from_yaml_str(&format!(
        r#"
schemaVersion: ledger.execution-plan.v1
id: cancel-test
version: 1
sourceBaseline: {{ sha256: "{}" }}
input: {{ promptTemplate: "go" }}
limits: {{ maxSteps: 20, maxReasonerCalls: 5, maxTokensTotal: 100000, timeoutS: 600, artifactMaxMb: 5 }}
onLimit: abort
models: {{ coder: {{ ctx: 8192 }} }}
mcp: {{ servers: [], policies: {{}} }}
entryStep: work
nodes:
  work:
    kind: task
    taskRef: flaky-task
    onVerified: done
    onError: {{ retry: {retry}, backoffS: {delay}, then: done }}
  done: {{ kind: end, outcome: succeeded }}
"#,
        "0".repeat(64)
    ))
    .unwrap()
}

struct Control {
    cancelled: AtomicBool,
    entered: Mutex<Option<Sender<()>>>,
    gate: Mutex<()>,
    wake: Condvar,
}

impl Control {
    fn new(entered: Option<Sender<()>>) -> Self {
        Self {
            cancelled: AtomicBool::new(false),
            entered: Mutex::new(entered),
            gate: Mutex::new(()),
            wake: Condvar::new(),
        }
    }

    fn cancel(&self) {
        let _guard = self.gate.lock().unwrap();
        self.cancelled.store(true, Ordering::SeqCst);
        self.wake.notify_all();
    }
}

impl ExecutionControl for Control {
    fn checkpoint(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    fn wait_backoff(&self, _: Duration) -> bool {
        if let Some(tx) = self.entered.lock().unwrap().take() {
            tx.send(()).unwrap();
        }
        let mut guard = self.gate.lock().unwrap();
        while !self.cancelled.load(Ordering::SeqCst) {
            guard = self.wake.wait(guard).unwrap();
        }
        true
    }
}

struct CountingModel {
    calls: Arc<AtomicUsize>,
    cancel_on_call: Option<Arc<Control>>,
    fail_on_cancel: bool,
}

impl ModelSlot for CountingModel {
    fn generate(&mut self, _: GenerateRequest) -> anyhow::Result<GenerateResponse> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(control) = &self.cancel_on_call {
            control.cancel();
            if self.fail_on_cancel {
                anyhow::bail!("model stopped");
            }
        }
        Ok(GenerateResponse {
            text: FENCE.into(),
            tokens_in: 1,
            tokens_out: 1,
            finish: FinishReason::Stop,
            capacity: None,
        })
    }
}

struct Gateway {
    inner: ScriptGateway,
    cancel_on_call: Option<Arc<Control>>,
    calls: Arc<AtomicUsize>,
    mutation: bool,
}

impl ToolGateway for Gateway {
    fn tool_class(&self, server: &str, tool: &str) -> trollm_mcp::policy::ToolClass {
        if self.mutation {
            trollm_mcp::policy::ToolClass::Mutation
        } else {
            self.inner.tool_class(server, tool)
        }
    }
    fn list_tools(&self) -> anyhow::Result<Vec<ToolDescriptor>> {
        self.inner.list_tools()
    }
    fn validate_call(&self, call: &ToolCall) -> anyhow::Result<()> {
        self.inner.validate_call(call)
    }
    fn call(&mut self, call: &ToolCall) -> anyhow::Result<ToolResult> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let result = self.inner.call(call);
        if let Some(control) = &self.cancel_on_call {
            control.cancel();
        }
        result
    }
}

fn executor(
    delay: u64,
    control: Arc<Control>,
    model_cancel: bool,
    tool_cancel: bool,
    mutation: bool,
    fail_model_on_cancel: bool,
) -> (
    Executor<CountingModel, Gateway>,
    std::path::PathBuf,
    Arc<AtomicUsize>,
    Arc<AtomicUsize>,
) {
    let base = common::tmp_dir("cancel");
    std::fs::write(base.join("prompt.md"), "do the thing").unwrap();
    let run_dir = RunDir::under(&base);
    let path = run_dir.path().to_path_buf();
    let model_calls = Arc::new(AtomicUsize::new(0));
    let tool_calls = Arc::new(AtomicUsize::new(0));
    let model = CountingModel {
        calls: model_calls.clone(),
        cancel_on_call: model_cancel.then(|| control.clone()),
        fail_on_cancel: fail_model_on_cancel,
    };
    let gateway = Gateway {
        inner: ScriptGateway {
            fail_always: if tool_cancel {
                Default::default()
            } else {
                ["t.flaky".to_string()].into()
            },
            ..Default::default()
        },
        cancel_on_call: tool_cancel.then(|| control.clone()),
        calls: tool_calls.clone(),
        mutation,
    };
    let registry = if mutation {
        REGISTRY.replace(
            "    loop:",
            "    mutations: { idempotency: intent-log, onUncertain: fail-run }\n    loop:",
        )
    } else {
        REGISTRY.to_string()
    };
    let mut exec = Executor::new(
        plan(delay, mutation),
        TaskRegistry::from_yaml_str(&registry, &base).unwrap(),
        String::new(),
        SlotMap::new().with("coder", model),
        gateway,
        run_dir,
    )
    .unwrap();
    exec.set_control(control);
    (exec, path, model_calls, tool_calls)
}

fn assert_cancelled(status: RunStatus, path: &std::path::Path) {
    assert_eq!(
        status,
        RunStatus::Failed {
            reason: "cancelled".into()
        }
    );
    let events = read_events(path);
    assert_eq!(events.last().unwrap()["outcome"], "cancelled");
    assert!(!events
        .iter()
        .any(|event| event["type"] == "transition" && event["key"] == "onError"));
    let snapshot: trollm_core::RunState =
        serde_json::from_slice(&std::fs::read(path.join("state.snapshot.json")).unwrap()).unwrap();
    assert_eq!(snapshot.terminal_reason.as_deref(), Some("cancelled"));
}

#[test]
fn long_backoff_cancel_wakes_before_retry() {
    let (tx, entered) = channel();
    let control = Arc::new(Control::new(Some(tx)));
    let (mut exec, path, model_calls, tool_calls) =
        executor(300, control.clone(), false, false, false, false);
    let (done_tx, done_rx) = channel();
    let worker = std::thread::spawn(move || done_tx.send(exec.run_until_blocked()).unwrap());
    entered
        .recv_timeout(Duration::from_secs(2))
        .expect("never entered backoff");
    control.cancel();
    let status = done_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("cancel did not wake backoff")
        .unwrap();
    worker.join().unwrap();
    assert_cancelled(status, &path);
    assert_eq!(model_calls.load(Ordering::SeqCst), 1);
    assert_eq!(tool_calls.load(Ordering::SeqCst), 1);
}

#[test]
fn zero_delay_and_model_tool_errors_bypass_authored_retry() {
    for (model_cancel, tool_cancel) in [(true, false), (false, true)] {
        let control = Arc::new(Control::new(None));
        let (mut exec, path, model_calls, tool_calls) =
            executor(0, control, model_cancel, tool_cancel, false, true);
        let status = exec.run_until_blocked().unwrap();
        assert_cancelled(status, &path);
        assert_eq!(model_calls.load(Ordering::SeqCst), 1);
        assert_eq!(tool_calls.load(Ordering::SeqCst), usize::from(tool_cancel));
        let events = read_events(&path);
        assert_eq!(
            events
                .iter()
                .filter(|event| event["type"] == "tool_call_result")
                .count(),
            usize::from(tool_cancel),
            "a successful tool result must be journaled before cancellation"
        );
    }
}

#[test]
fn successful_model_response_cancel_stops_before_tool_dispatch() {
    let control = Arc::new(Control::new(None));
    let (mut exec, path, model_calls, tool_calls) = executor(0, control, true, false, false, false);
    assert_cancelled(exec.run_until_blocked().unwrap(), &path);
    assert_eq!(model_calls.load(Ordering::SeqCst), 1);
    assert_eq!(tool_calls.load(Ordering::SeqCst), 0);
    let events = read_events(&path);
    assert_eq!(
        events.iter().filter(|e| e["type"] == "model_call").count(),
        1
    );
    assert!(!events.iter().any(|e| e["type"] == "tool_call_intent"));
}

#[test]
fn successful_mutation_result_is_linked_before_cancel() {
    let control = Arc::new(Control::new(None));
    let (mut exec, path, model_calls, tool_calls) = executor(0, control, false, true, true, false);
    assert_cancelled(exec.run_until_blocked().unwrap(), &path);
    assert_eq!(model_calls.load(Ordering::SeqCst), 1);
    assert_eq!(tool_calls.load(Ordering::SeqCst), 1);
    let events = read_events(&path);
    let intent = events
        .iter()
        .find(|e| e["type"] == "tool_call_intent")
        .unwrap();
    let result = events
        .iter()
        .find(|e| e["type"] == "tool_call_result")
        .unwrap();
    assert_eq!(result["intentSeq"], intent["seq"]);
    assert!(result["seq"].as_u64().unwrap() < events.last().unwrap()["seq"].as_u64().unwrap());
}
