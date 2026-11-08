//! Real gateway/wire controls, not model or catalog business acceptance.
mod common;
use anyhow::{ensure, Result};
use common::TempDir;
#[path = "common/tls_mock.rs"]
mod tls_mock;
use serde_json::json;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use tls_mock::TlsMcpServer as MockMcpServer;
use trollm_mcp::preparation::{
    raw_sha256, CallPreparer, PreparationContext, PreparationEvidence, PreparedCall,
};
use trollm_mcp::transport::{McpClient, TokenGetter};
use trollm_mcp::{CallContext, McpGateway, PolicySet, ServerConfig, ToolCall, ToolGateway};

struct Guard {
    calls: AtomicUsize,
    delay: bool,
    cancel: Arc<AtomicBool>,
}
impl CallPreparer for Guard {
    // Controlled gateway-port attestation only. Native journal authentication
    // is exercised by the real selected provider's separate controls.
    fn after_retained_intent(
        &self,
        _: &ToolCall,
        _: &PreparationContext,
        _: &PreparationEvidence,
        seq: u64,
    ) -> Result<()> {
        ensure!(seq > 0, "controlled native intent required");
        Ok(())
    }
    fn prepare(
        &self,
        call: &ToolCall,
        context: &PreparationContext,
        _: Option<Instant>,
    ) -> Result<PreparedCall> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.delay {
            std::thread::sleep(Duration::from_millis(20));
        }
        let response = "{\"args\":{\"id\":3},\"fixtureOnly\":true}";
        Ok(PreparedCall {
            call: ToolCall {
                args: json!({"id":3}),
                ..call.clone()
            },
            evidence: PreparationEvidence {
                provider: "controlled-private-guard".into(),
                context_sha256: context.raw()?.sha256,
                response_json: response.into(),
                response_sha256: raw_sha256(response.as_bytes()),
            },
        })
    }
    fn check_before_send(
        &self,
        _: &ToolCall,
        _: &PreparationContext,
        _: &PreparationEvidence,
    ) -> Result<()> {
        ensure!(
            !self.cancel.load(Ordering::SeqCst),
            "controlled abort at write"
        );
        Ok(())
    }
}
fn context(call: u32) -> PreparationContext {
    PreparationContext {
        run_id: "controlled-run".into(),
        node: "work".into(),
        task_ref: "work".into(),
        model_slot: "coder".into(),
        model_call_ordinal: 1,
        frames: vec![],
        cursors: vec![],
        argument_bindings: vec![],
        call_context: CallContext {
            epoch: "controlled-native-epoch".into(),
            attempt: u64::MAX,
            call,
        },
    }
}
fn gateway(
    server: &MockMcpServer,
    tool: &str,
    dirs: &TempDir,
    guard: Arc<Guard>,
    cancel: Arc<AtomicBool>,
    cancel_on_token: Arc<AtomicBool>,
) -> McpGateway {
    let token_cancel = cancel.clone();
    let server = ServerConfig::for_local_test("tracker", &server.url)
        .with_root_certificates(
            trollm_mcp::RootCertificates::from_pem(common::tls_fixture::ROOT_1_PEM).unwrap(),
        )
        .with_grant_token_getter(Arc::new(move || {
            if cancel_on_token.load(Ordering::SeqCst) {
                token_cancel.store(true, Ordering::SeqCst);
            }
            Some("fixture-only-token".into())
        }))
        .with_protocol_version("2025-03-26")
        .unwrap();
    McpGateway::new(
        vec![server],
        PolicySet::from_yaml_str(&format!(
            "tracker.{tool}: {{class: mutation, passthrough: true}}"
        ))
        .unwrap(),
        dirs.path().join("cache"),
        dirs.path().join("artifacts"),
        None,
    )
    .unwrap()
    .with_preparer(vec![format!("tracker.{tool}")], guard)
    .unwrap()
    .with_send_gate(Arc::new(move || {
        ensure!(!cancel.load(Ordering::SeqCst), "native cancellation");
        Ok(())
    }))
}

#[test]
fn preparation_without_native_intent_never_dispatches_and_fences_its_permit() {
    let server = MockMcpServer::start();
    let dirs = TempDir::new("prepared-without-native-intent");
    let cancel = Arc::new(AtomicBool::new(false));
    let guard = Arc::new(Guard {
        calls: AtomicUsize::new(0),
        delay: false,
        cancel: cancel.clone(),
    });
    let mut native = gateway(
        &server,
        "close_issue",
        &dirs,
        guard,
        cancel,
        Arc::new(AtomicBool::new(false)),
    );
    let ctx = context(1);
    native.set_call_context(Some(ctx.call_context.clone()));
    let call = ToolCall {
        server: "tracker".into(),
        tool: "close_issue".into(),
        args: json!({}),
    };
    let prepared = native.prepare_call(&call, &ctx).unwrap().unwrap();
    let error = native.call(&prepared.call).unwrap_err();
    assert!(error.to_string().contains("native intent"));
    assert_eq!(server.count("call:close_issue"), 0);
    assert!(native
        .after_retained_intent(&prepared.call, &ctx, Some(4))
        .is_err());
    assert!(native.call(&prepared.call).is_err());
    assert_eq!(server.count("call:close_issue"), 0);
}

#[test]
fn missing_changed_duplicate_preparation_fails_closed_and_exact_call_sends_once() {
    let server = MockMcpServer::start();
    let dirs = TempDir::new("mandatory-preparation");
    let cancel = Arc::new(AtomicBool::new(false));
    let guard = Arc::new(Guard {
        calls: AtomicUsize::new(0),
        delay: false,
        cancel: cancel.clone(),
    });
    let mut native = gateway(
        &server,
        "close_issue",
        &dirs,
        guard.clone(),
        cancel,
        Arc::new(AtomicBool::new(false)),
    );
    let requested = ToolCall {
        server: "tracker".into(),
        tool: "close_issue".into(),
        args: json!({"purpose":"controlled"}),
    };
    assert!(native.validate_call(&requested).is_err());
    assert!(native.call(&requested).is_err());
    assert_eq!(server.count("initialize"), 0);
    let ctx = context(1);
    native.set_call_context(Some(ctx.call_context.clone()));
    let prepared = native.prepare_call(&requested, &ctx).unwrap().unwrap();
    native
        .after_retained_intent(&prepared.call, &ctx, Some(4))
        .unwrap();
    let mut changed = prepared.call.clone();
    changed.args["id"] = json!(4);
    assert!(native.call(&changed).is_err());
    assert!(
        native.call(&prepared.call).is_err(),
        "bad dispatch consumed its permit"
    );
    assert!(
        native.prepare_call(&requested, &ctx).is_err(),
        "native context cannot be reused"
    );
    assert!(
        native.prepare_call(&requested, &context(2)).is_err(),
        "failed selected dispatch fences even a fresh context"
    );
    let fresh_dirs = TempDir::new("mandatory-preparation-fresh");
    let cancel = Arc::new(AtomicBool::new(false));
    let mut native = gateway(
        &server,
        "close_issue",
        &fresh_dirs,
        guard.clone(),
        cancel,
        Arc::new(AtomicBool::new(false)),
    );
    let ctx = context(2);
    native.set_call_context(Some(ctx.call_context.clone()));
    let prepared = native.prepare_call(&requested, &ctx).unwrap().unwrap();
    native
        .after_retained_intent(&prepared.call, &ctx, Some(4))
        .unwrap();
    native.validate_call(&prepared.call).unwrap();
    let result = native.call(&prepared.call).unwrap();
    assert!(
        native.prepare_call(&requested, &context(3)).is_err(),
        "returned result cannot advance before retained acknowledgement"
    );
    assert!(
        native
            .after_retained_result(&prepared.call, &ctx, &result, Some(4), 5)
            .is_err(),
        "provider without authentic retained-result support refuses"
    );
    assert!(native.call(&prepared.call).is_err());
    assert_eq!(server.count("call:close_issue"), 1);
    assert_eq!(guard.calls.load(Ordering::SeqCst), 2);
}

#[test]
fn deadline_after_guard_and_cancellation_during_credentials_produce_zero_effect_calls() {
    for late in [false, true] {
        let server = MockMcpServer::start();
        let dirs = TempDir::new("late-preparation");
        let cancel = Arc::new(AtomicBool::new(false));
        let arm = Arc::new(AtomicBool::new(false));
        let guard = Arc::new(Guard {
            calls: AtomicUsize::new(0),
            delay: late,
            cancel: cancel.clone(),
        });
        let mut gateway = gateway(&server, "close_issue", &dirs, guard, cancel, arm.clone());
        gateway.list_tools().unwrap();
        let ctx = context(1);
        gateway.set_call_context(Some(ctx.call_context.clone()));
        let requested = ToolCall {
            server: "tracker".into(),
            tool: "close_issue".into(),
            args: json!({}),
        };
        if late {
            gateway.set_deadline(Some(Instant::now() + Duration::from_millis(5)));
        }
        let prepared = gateway.prepare_call(&requested, &ctx);
        if late {
            assert!(prepared.is_err());
        } else {
            let prepared = prepared.unwrap().unwrap();
            gateway
                .after_retained_intent(&prepared.call, &ctx, Some(4))
                .unwrap();
            arm.store(true, Ordering::SeqCst);
            assert!(gateway.call(&prepared.call).is_err());
        }
        assert_eq!(server.count("call:close_issue"), 0);
    }
}

#[test]
fn response_loss_has_one_possible_send_and_no_permit_reuse() {
    let server = MockMcpServer::start();
    let dirs = TempDir::new("prepared-response-loss");
    let cancel = Arc::new(AtomicBool::new(false));
    let guard = Arc::new(Guard {
        calls: AtomicUsize::new(0),
        delay: false,
        cancel: cancel.clone(),
    });
    let mut gateway = gateway(
        &server,
        "drop_mutation",
        &dirs,
        guard,
        cancel,
        Arc::new(AtomicBool::new(false)),
    );
    let ctx = context(1);
    gateway.set_call_context(Some(ctx.call_context.clone()));
    let requested = ToolCall {
        server: "tracker".into(),
        tool: "drop_mutation".into(),
        args: json!({}),
    };
    let prepared = gateway.prepare_call(&requested, &ctx).unwrap().unwrap();
    gateway
        .after_retained_intent(&prepared.call, &ctx, Some(4))
        .unwrap();
    assert!(gateway.call(&prepared.call).is_err());
    assert!(gateway.call(&prepared.call).is_err());
    assert_eq!(server.count("call:drop_mutation"), 1);
}

#[test]
fn actual_transport_checks_after_token_callback_and_negotiates_selected_protocol() {
    let server = MockMcpServer::start_with_protocol("2025-06-18");
    let arm = Arc::new(AtomicBool::new(false));
    let cancelled = Arc::new(AtomicBool::new(false));
    let a = arm.clone();
    let c = cancelled.clone();
    let token: TokenGetter = Arc::new(move || {
        if a.load(Ordering::SeqCst) {
            c.store(true, Ordering::SeqCst);
        }
        Some("fixture-only".into())
    });
    let mut client = McpClient::with_root_certificates(
        &server.url,
        Some(token),
        trollm_mcp::RootCertificates::from_pem(common::tls_fixture::ROOT_1_PEM).unwrap(),
    )
    .unwrap()
    .with_protocol_version("2025-06-18")
    .unwrap();
    client.list_tools(Some(Duration::from_secs(1))).unwrap();
    let check = || {
        ensure!(!cancelled.load(Ordering::SeqCst), "cancel after refresh");
        Ok(())
    };
    arm.store(true, Ordering::SeqCst);
    assert!(client
        .call_tool_checked(
            "close_issue",
            &json!({"id":3}),
            Some(Duration::from_secs(1)),
            Some(&check)
        )
        .is_err());
    assert_eq!(server.count("call:close_issue"), 0);
    arm.store(false, Ordering::SeqCst);
    cancelled.store(false, Ordering::SeqCst);
    client
        .call_tool_checked(
            "close_issue",
            &json!({"id":3}),
            Some(Duration::from_secs(1)),
            Some(&check),
        )
        .unwrap();
    assert_eq!(server.count("call:close_issue"), 1);
    let wrong = MockMcpServer::start();
    let mut client = McpClient::with_root_certificates(
        &wrong.url,
        None,
        trollm_mcp::RootCertificates::from_pem(common::tls_fixture::ROOT_1_PEM).unwrap(),
    )
    .unwrap()
    .with_protocol_version("2025-06-18")
    .unwrap();
    assert!(client.list_tools(Some(Duration::from_secs(1))).is_err());
    assert_eq!(wrong.count("list"), 0);
}
