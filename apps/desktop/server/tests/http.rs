use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::ServiceExt;
use trollm_core::plan::hex_sha256;
use trollm_desktop::{router, Host, Origins};

const ORIGIN: &str = "http://127.0.0.1:8080";
const AUTHORITY: &str = "127.0.0.1:8080";
const PASSPHRASE: &str = "integration test passphrase";
const START_ID: &str = "4f40c9d9-9d34-4ec3-8708-6453743ec071";
const PENDING_ID: &str = "4f40c9d9-9d34-4ec3-8708-6453743ec073";
const JOURNAL_ID: &str = "4f40c9d9-9d34-4ec3-8708-6453743ec075";
const FIXTURE_ENDPOINT: &str = "https://example.invalid/mcp";

fn starters() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../android/app/src/main/assets/flows")
}

fn open(path: &Path) -> Router {
    let origins = Origins::new(ORIGIN, Some("http://localhost:8080")).unwrap();
    let path = path.to_owned();
    let host = std::thread::spawn(move || {
        Host::open(&path, &starters(), format!("{ORIGIN}/oauth/callback")).unwrap()
    })
    .join()
    .unwrap();
    router(host, origins, PathBuf::new())
}

fn request(method: &str, path: &str, token: Option<&str>, value: Option<Value>) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, AUTHORITY);
    if let Some(token) = token {
        request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let body = if let Some(value) = value {
        request = request.header(header::CONTENT_TYPE, "application/json");
        Body::from(value.to_string())
    } else {
        Body::empty()
    };
    request.body(body).unwrap()
}

async fn send(app: &Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&body).unwrap_or_else(|_| panic!("expected JSON for {status}")),
    )
}

async fn login(app: &Router) -> String {
    let (status, value) = send(
        app,
        request(
            "POST",
            "/api/session",
            None,
            Some(json!({"passphrase":PASSPHRASE})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    value["token"].as_str().unwrap().to_owned()
}

fn bundle(selected: bool, pausing: bool) -> (String, Vec<u8>) {
    calculation_bundle(selected, pausing, None)
}

fn calculation_bundle(
    selected: bool,
    pausing: bool,
    bearer_endpoint: Option<&str>,
) -> (String, Vec<u8>) {
    let mut setup = json!({"format":"trollm.flow-setup.v1","title":"Direct calculation fixture","models":{},"input":{"fields":[{"key":"left","label":"Left","required":true},{"key":"right","label":"Right","required":true},{"key":"budget","label":"Budget","required":true}]}});
    let mut plan =
        include_str!("../../../../flows/examples/direct-calculation.flow.yaml").to_owned();
    if let Some(endpoint) = bearer_endpoint {
        setup["connections"] = json!({"fixture":{"url":endpoint,"auth":"bearer"}});
        plan = plan.replace(
            "servers: []",
            &format!("servers: [{{name: fixture, url: '{endpoint}'}}]"),
        );
    }
    let setup = setup.to_string();
    let mut flow = format!(
        "{}\nsetup: {{path: setup.json, sha256: {}}}\n",
        plan,
        hex_sha256(setup.as_bytes()),
    );
    if pausing {
        flow = flow.replace(
            "onVerified: compare",
            "onVerified: {status: paused, resumeAt: compare}",
        );
    }
    let mut files: Vec<(&str, &[u8])> = vec![
        ("direct-calculation.flow.yaml", flow.as_bytes()),
        (
            "direct-calculation.tasks.yaml",
            include_bytes!("../../../../flows/tasks/direct-calculation.tasks.yaml"),
        ),
        ("setup.json", setup.as_bytes()),
    ];
    if selected {
        files.push(("collection-profile.json", b"{}"));
    }
    (hex_sha256(flow.as_bytes()), archive(files))
}

fn archive(files: Vec<(&str, &[u8])>) -> Vec<u8> {
    let manifest = json!({
        "format": "trollm.flow-bundle.v1",
        "entryFlow": files[0].0,
        "taskRegistry": files[1].0,
        "files": files.iter().map(|(path, bytes)| json!({"path": path, "sha256":hex_sha256(bytes)})).collect::<Vec<_>>(),
    }).to_string();
    let mut tar = tar::Builder::new(Vec::new());
    let mut append = |path: &str, bytes: &[u8]| {
        let mut header = tar::Header::new_ustar();
        header.set_mode(0o644);
        header.set_size(bytes.len() as u64);
        tar.append_data(&mut header, path, bytes).unwrap();
    };
    for (path, bytes) in files {
        append(path, bytes);
    }
    append("manifest.json", manifest.as_bytes());
    tar.into_inner().unwrap()
}

fn import_request(token: &str, bytes: Vec<u8>) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/api/flows/import")
        .header(header::HOST, AUTHORITY)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/x-tar")
        .body(Body::from(bytes))
        .unwrap()
}

async fn wait_for_run(app: &Router, token: &str, id: &Value) -> Value {
    let uri = format!("/api/runs/{}?after=0", id.as_str().unwrap());
    for _ in 0..200 {
        let (status, value) = send(app, request("GET", &uri, Some(token), None)).await;
        assert_eq!(status, StatusCode::OK, "{value}");
        if value["run"]["workerActive"] == false {
            return value;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("fixture did not stop within four seconds")
}

#[tokio::test]
async fn rejects_cross_origin_and_requires_an_active_local_session() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desktop");
    let app = open(&path);
    assert!(Host::open(&path, &starters(), format!("{ORIGIN}/oauth/callback")).is_err());

    for (host, origin) in [
        ("rebinding.example", None),
        (AUTHORITY, Some("https://other.example")),
        (AUTHORITY, Some("http://localhost:8080")),
        (AUTHORITY, Some("null")),
    ] {
        let mut probe = request("GET", "/api/bootstrap", None, None);
        probe
            .headers_mut()
            .insert(header::HOST, host.parse().unwrap());
        if let Some(origin) = origin {
            probe
                .headers_mut()
                .insert(header::ORIGIN, origin.parse().unwrap());
        }
        let (status, value) = send(&app, probe).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(value["error"]["code"], "ORIGIN_DENIED");
    }
    let mut alias = request("GET", "/api/bootstrap", None, None);
    alias
        .headers_mut()
        .insert(header::HOST, "localhost:8080".parse().unwrap());
    alias
        .headers_mut()
        .insert(header::ORIGIN, "http://localhost:8080".parse().unwrap());
    assert_eq!(send(&app, alias).await.1["needsSetup"], true);
    assert_eq!(
        send(&app, request("GET", "/api/flows", None, None)).await.0,
        StatusCode::UNAUTHORIZED
    );
    let token = login(&app).await;
    let other = login(&app).await;
    let (status, flows) = send(&app, request("GET", "/api/flows", Some(&token), None)).await;
    assert_eq!(status, StatusCode::OK, "{flows}");
    assert_eq!(flows["flows"].as_array().unwrap().len(), 3);
    assert_eq!(
        send(&app, request("DELETE", "/api/session", Some(&token), None))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        send(&app, request("GET", "/api/flows", Some(&token), None))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(&app, request("GET", "/api/flows", Some(&other), None))
            .await
            .0,
        StatusCode::OK
    );
    let (status, value) = send(
        &app,
        request(
            "POST",
            "/api/session",
            None,
            Some(json!({"passphrase":"different secret"})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(!value.to_string().contains("different secret"));

    let callback_url = format!(
        "/oauth/callback/{}?state=bad-state&code=private-code&iss=https%3A%2F%2Fexample.invalid",
        "a".repeat(64),
    );
    let head = app
        .clone()
        .oneshot(request("HEAD", &callback_url, None, None))
        .await
        .unwrap();
    assert_eq!(head.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert!(!head.headers().contains_key(header::LOCATION));
    let callback = app
        .oneshot(request("GET", &callback_url, None, None))
        .await
        .unwrap();
    assert_eq!(callback.status(), StatusCode::SEE_OTHER);
    assert_eq!(callback.headers()[header::LOCATION], "/?oauth=failed");
    assert_eq!(callback.headers()[header::REFERRER_POLICY], "no-referrer");
}

#[tokio::test]
async fn starts_verified_direct_flow_once_and_fences_pending_receipts_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desktop");
    let app = open(&path);
    let token = login(&app).await;

    let (_, selected) = bundle(true, false);
    let (status, error) = send(&app, import_request(&token, selected)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(error["error"]["code"], "UNSUPPORTED_PROFILE");

    let (sha, archive) = bundle(false, false);
    let (status, imported) = send(&app, import_request(&token, archive)).await;
    assert_eq!(status, StatusCode::OK, "{imported}");
    assert_eq!(imported["flow"]["sha256"], sha);
    let detail_url = format!("/api/flows/{sha}");
    let (_, detail) = send(&app, request("GET", &detail_url, Some(&token), None)).await;
    assert_eq!(detail["canStart"], true, "{detail}");
    let (_, inspect) = send(
        &app,
        request("GET", &format!("{detail_url}/inspect"), Some(&token), None),
    )
    .await;
    assert_eq!(inspect["plans"][0]["nodes"]["sum"]["taskRef"], "add-inputs");
    assert_eq!(
        inspect["registry"]["tasks"]["add-inputs"]["tools"][0],
        "local.calculate"
    );

    let invalid = json!({"requestId":START_ID,"flowSha256":sha,"input":"{}"});
    let (status, error) = send(
        &app,
        request("POST", "/api/runs", Some(&token), Some(invalid)),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(error["error"]["code"], "INVALID_INPUT");
    assert_eq!(
        fs::read_dir(path.join("start-requests")).unwrap().count(),
        0
    );
    assert_eq!(fs::read_dir(path.join("trollm/runs")).unwrap().count(), 0);

    let calculating = json!({"left":"39.99","right":"40.10","budget":"80"}).to_string();
    let start = json!({"requestId":START_ID,"flowSha256":sha,"input":calculating});
    let first = send(
        &app,
        request("POST", "/api/runs", Some(&token), Some(start.clone())),
    );
    let duplicate = send(
        &app,
        request("POST", "/api/runs", Some(&token), Some(start.clone())),
    );
    let ((first_status, first), (second_status, duplicate)) = tokio::join!(first, duplicate);
    assert_eq!(first_status, StatusCode::OK, "{first}");
    assert_eq!(second_status, StatusCode::OK, "{duplicate}");
    assert_eq!(first["id"], duplicate["id"]);
    let result = wait_for_run(&app, &token, &first["id"]).await;
    assert_eq!(result["run"]["phase"], "succeeded", "{result}");
    assert_eq!(result["run"]["flowSha256"], sha);
    assert_eq!(result["hasMore"], false);
    assert_eq!(result["nextSeq"], result["run"]["lastSeq"]);
    assert!(result["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|event| event["type"] == "direct_tool_step"));
    assert_eq!(fs::read_dir(path.join("trollm/runs")).unwrap().count(), 1);
    let mut conflict = start.clone();
    conflict["input"] = json!("different input");
    let (status, value) = send(
        &app,
        request("POST", "/api/runs", Some(&token), Some(conflict)),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(value["error"]["code"], "REQUEST_ID_CONFLICT");
    drop(app);

    let fingerprint = hex_sha256(format!("{sha}{calculating}").as_bytes());
    fs::write(
        path.join("start-requests").join(PENDING_ID),
        json!({
            "fingerprint": fingerprint,
            "state": {"outcome":"pending"},
        })
        .to_string(),
    )
    .unwrap();
    let app = open(&path);
    assert_eq!(
        send(&app, request("GET", "/api/runs", Some(&token), None))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    let token = login(&app).await;
    let (status, replay) = send(
        &app,
        request("POST", "/api/runs", Some(&token), Some(start)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay["id"], first["id"]);
    let uncertain = json!({"requestId":PENDING_ID,"flowSha256":sha,"input":calculating});
    let (status, error) = send(
        &app,
        request("POST", "/api/runs", Some(&token), Some(uncertain)),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error["error"]["code"], "START_UNCERTAIN");
    let (_, history) = send(&app, request("GET", "/api/runs", Some(&token), None)).await;
    assert_eq!(history["runs"].as_array().unwrap().len(), 1);
    assert_eq!(history["runs"][0]["id"], first["id"]);
}

#[tokio::test]
async fn actual_worker_gates_a_second_run_independently_of_saved_events() {
    use trollm_core::{EventLog, EventType};

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desktop");
    let app = open(&path);
    let token = login(&app).await;
    let (sha, archive) = bundle(false, true);
    let (status, imported) = send(&app, import_request(&token, archive)).await;
    assert_eq!(status, StatusCode::OK, "{imported}");
    let calculating = json!({"left":"39.99","right":"40.10","budget":"80"}).to_string();
    let start = json!({"requestId":START_ID,"flowSha256":sha,"input":calculating});
    let (status, started) = send(
        &app,
        request("POST", "/api/runs", Some(&token), Some(start)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{started}");
    let id = started["id"].as_str().unwrap();
    let uri = format!("/api/runs/{id}");
    let mut paused = false;
    for _ in 0..200 {
        let (_, view) = send(&app, request("GET", &uri, Some(&token), None)).await;
        if view["run"]["phase"] == "paused" {
            assert_eq!(view["run"]["workerActive"], true);
            assert_eq!(view["run"]["canResume"], true);
            paused = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(paused, "expected a live authored pause");

    let next_start = json!({"requestId":PENDING_ID,"flowSha256":sha,"input":calculating});
    let (status, value) = send(
        &app,
        request("POST", "/api/runs", Some(&token), Some(next_start.clone())),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(value["error"]["code"], "RUN_ACTIVE");
    let (status, resumed) = send(
        &app,
        request(
            "POST",
            &format!("{uri}/action"),
            Some(&token),
            Some(json!({ "action":"resume" })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{resumed}");
    let finished = wait_for_run(&app, &token, &started["id"]).await;
    assert_eq!(finished["run"]["phase"], "succeeded");
    assert_eq!(finished["run"]["canResume"], false);

    let synthetic_run = path.join("trollm/runs").join(JOURNAL_ID);
    fs::create_dir(&synthetic_run).unwrap();
    let mut log = EventLog::open(&synthetic_run.join("events.jsonl"), JOURNAL_ID).unwrap();
    for _ in 0..400 {
        log.append(
            EventType::NodeEntered,
            json!({"node":"example", "detail":"private-tool-argument"}),
        )
        .unwrap();
    }
    drop(log);
    let uri = format!("/api/runs/{JOURNAL_ID}");
    let (status, first) = send(&app, request("GET", &uri, Some(&token), None)).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["events"].as_array().unwrap().len(), 129);
    assert_eq!(first["events"][0]["type"], "events_omitted");
    assert_eq!(first["hasMore"], true);
    assert!(!first.to_string().contains("private-tool-argument"));
    let second_uri = format!("{uri}?after={}", first["nextSeq"]);
    let (_, second) = send(&app, request("GET", &second_uri, Some(&token), None)).await;
    assert_eq!(second["events"].as_array().unwrap().len(), 128);
    assert_eq!(second["hasMore"], false);
    assert_eq!(second["nextSeq"], 400);

    // Evict the completed run's cached observation. Its corrupt journal must
    // make that run unreadable without blocking admission for another run.
    let runs = path.join("trollm/runs");
    for index in 0..16 {
        let other_id = format!("b05aa43f-2f82-4643-9f7a-{index:012x}");
        fs::create_dir(runs.join(&other_id)).unwrap();
        fs::write(runs.join(&other_id).join("events.jsonl"), b"").unwrap();
        let other_uri = format!("/api/runs/{other_id}");
        assert_eq!(
            send(&app, request("GET", &other_uri, Some(&token), None))
                .await
                .0,
            StatusCode::OK
        );
    }
    fs::write(runs.join(id).join("events.jsonl"), b"not a run journal\n").unwrap();
    let old_uri = format!("/api/runs/{id}");
    let (status, _) = send(&app, request("GET", &old_uri, Some(&token), None)).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let flow_uri = format!("/api/flows/{sha}");
    let (status, ready) = send(&app, request("GET", &flow_uri, Some(&token), None)).await;
    assert_eq!(status, StatusCode::OK, "{ready}");
    assert_eq!(ready["canStart"], true);
    let (status, next) = send(
        &app,
        request("POST", "/api/runs", Some(&token), Some(next_start)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{next}");
    assert_ne!(next["id"], started["id"]);
    let cancel_uri = format!("/api/runs/{}/action", next["id"].as_str().unwrap());
    let (status, cancelled) = send(
        &app,
        request(
            "POST",
            &cancel_uri,
            Some(&token),
            Some(json!({"action":"cancel"})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{cancelled}");
    assert_eq!(
        wait_for_run(&app, &token, &next["id"]).await["run"]["canResume"],
        false
    );
}

#[cfg(all(feature = "stub", not(feature = "llama")))]
#[tokio::test]
async fn returns_the_shared_runtimes_saved_report_with_the_stub_backend() {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desktop");
    let app = open(&path);
    let token = login(&app).await;

    // The desktop verifies its model pin even with the deterministic test
    // backend. This fixture exists only for the stub and needs no GGUF weights.
    let model = b"synthetic pinned bytes for the stub";
    let model_sha = hex_sha256(model);
    let file = path
        .join("models")
        .join(format!("{model_sha}-{}.gguf", model.len()));
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(file)
        .unwrap()
        .write_all(model)
        .unwrap();
    let setup = json!({
        "format":"trollm.flow-setup.v1",
        "title":"Stub summary",
        "models":{"coder":{"label":"Stub","url":"https://example.invalid/model.gguf","sha256":model_sha,"sizeBytes":model.len(),"ctx":4096}}
    }).to_string();
    let flow = format!(
        "{}\nsetup: {{path: setup.json, sha256: {}}}\n",
        include_str!("../../../../flows/examples/minimal-summary.flow.yaml"),
        hex_sha256(setup.as_bytes()),
    );
    let sha = hex_sha256(flow.as_bytes());
    let fixture = archive(vec![
        ("minimal-summary.flow.yaml", flow.as_bytes()),
        (
            "minimal-summary.tasks.yaml",
            include_bytes!("../../../../flows/tasks/minimal-summary.tasks.yaml"),
        ),
        ("prompts/minimal-summary.md", b"Summarize {{ user_input }}"),
        ("setup.json", setup.as_bytes()),
    ]);
    let (status, value) = send(&app, import_request(&token, fixture)).await;
    assert_eq!(status, StatusCode::OK, "{value}");
    let mut verified = false;
    for _ in 0..200 {
        let (status, value) = send(
            &app,
            request("GET", &format!("/api/flows/{sha}"), Some(&token), None),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{value}");
        if value["canStart"] == true {
            verified = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(verified, "stub model fixture did not verify");
    let start = json!({"requestId":START_ID,"flowSha256":sha,"input":"A short article."});
    let (status, started) = send(
        &app,
        request("POST", "/api/runs", Some(&token), Some(start)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{started}");
    let view = wait_for_run(&app, &token, &started["id"]).await;
    assert_eq!(view["run"]["phase"], "succeeded", "{view}");
    assert_eq!(view["run"]["report"], "stub model response");
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir(destination).unwrap();
    fs::set_permissions(destination, fs::metadata(source).unwrap().permissions()).unwrap();
    for entry in fs::read_dir(source).unwrap().map(Result::unwrap) {
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else if entry.file_type().unwrap().is_file() {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

async fn save_fixture_bearer(app: &Router, session: &str) {
    let (status, value) = send(
        app,
        request(
            "PUT",
            "/api/connections/fixture",
            Some(session),
            Some(json!({"mode":"bearer","url":FIXTURE_ENDPOINT,"token":"test-bearer-secret"})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    assert!(!value.to_string().contains("test-bearer-secret"));
}

#[tokio::test]
async fn cold_resume_uses_the_original_manual_generation_and_refuses_a_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let live = dir.path().join("live");
    let app = open(&live);
    let token = login(&app).await;
    save_fixture_bearer(&app, &token).await;
    let (sha, archive) = calculation_bundle(false, true, Some(FIXTURE_ENDPOINT));
    let (status, imported) = send(&app, import_request(&token, archive)).await;
    assert_eq!(status, StatusCode::OK, "{imported}");
    let start = json!({
        "requestId": START_ID,
        "flowSha256": sha,
        "input": r#"{"left":"39.99","right":"40.10","budget":"80"}"#,
    });
    let (status, started) = send(
        &app,
        request("POST", "/api/runs", Some(&token), Some(start)),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{started}");
    let id = started["id"].as_str().unwrap();
    let uri = format!("/api/runs/{id}");
    let action = format!("{uri}/action");
    let mut paused = false;
    for _ in 0..200 {
        let (status, view) = send(&app, request("GET", &uri, Some(&token), None)).await;
        assert_eq!(status, StatusCode::OK, "{view}");
        if view["run"]["phase"] == "paused" {
            paused = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(paused, "fixture did not pause");

    // Copy the durable, parked run into two independent host roots. Each
    // reopened host has a fresh worker registry but the same persisted pins.
    let same = dir.path().join("same");
    let changed = dir.path().join("changed");
    copy_tree(&live, &same);
    copy_tree(&live, &changed);
    let identity_path = Path::new("run-identities").join(id);
    let identity = fs::read_to_string(changed.join(&identity_path)).unwrap();
    assert!(!identity.contains("test-bearer-secret"));
    let (status, value) = send(
        &app,
        request(
            "POST",
            &action,
            Some(&token),
            Some(json!({"action":"cancel"})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    let terminal = wait_for_run(&app, &token, &started["id"]).await;
    assert_eq!(terminal["run"]["error"], "Cancelled.");
    assert_eq!(terminal["run"]["canResume"], false);
    drop(app);

    // Simulate loss of a worker after saving its recoverable snapshot.
    fs::write(
        same.join("trollm/runs")
            .join(id)
            .join(trollm_uniffi::run::WORKER_FAILURE_FILE),
        br#"{"reason":"synthetic interrupted worker"}"#,
    )
    .unwrap();
    let app = open(&same);
    let token = login(&app).await;
    let (_, view) = send(&app, request("GET", &uri, Some(&token), None)).await;
    assert_eq!(view["run"]["phase"], "failed");
    assert_eq!(view["run"]["workerActive"], false);
    assert_eq!(view["run"]["canResume"], true);
    let resume = json!({"action":"resume"});
    let (status, value) = send(
        &app,
        request("POST", &action, Some(&token), Some(resume.clone())),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{value}");
    assert_eq!(
        wait_for_run(&app, &token, &started["id"]).await["run"]["phase"],
        "succeeded"
    );

    let app = open(&changed);
    let token = login(&app).await;
    let removed = request("DELETE", "/api/connections/fixture", Some(&token), None);
    assert_eq!(send(&app, removed).await.0, StatusCode::OK);
    let (status, value) = send(
        &app,
        request("POST", &action, Some(&token), Some(resume.clone())),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(value["error"]["code"], "RUN_CREDENTIALS_CHANGED");

    // Saving the same bytes again still establishes a new credential identity.
    save_fixture_bearer(&app, &token).await;
    let (status, value) = send(
        &app,
        request("POST", &action, Some(&token), Some(resume.clone())),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(value["error"]["code"], "RUN_CREDENTIALS_CHANGED");
    let (_, view) = send(&app, request("GET", &uri, Some(&token), None)).await;
    assert_eq!(view["run"]["phase"], "paused");
    assert_eq!(view["run"]["workerActive"], false);
    assert_eq!(view["run"]["canResume"], false);

    fs::remove_file(changed.join(identity_path)).unwrap();
    let (status, value) = send(&app, request("POST", &action, Some(&token), Some(resume))).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(value["error"]["code"], "RUN_IDENTITY_UNAVAILABLE");
}
