use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use tower::ServiceExt;
use trollm_flow_review::http::{router, Config};
#[tokio::test]
async fn health_catalog_source_upload_and_safe_json_limits() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config {
        packages: dir.path().join("packages"),
        temp: dir.path().join("staging"),
        static_dir: dir.path().join("web"),
    };
    std::fs::create_dir_all(&config.static_dir).unwrap();
    std::fs::write(config.static_dir.join("index.html"), "<html>Review</html>").unwrap();
    let app = router(config.clone());
    for (url, status) in [
        ("/healthz", StatusCode::OK),
        ("/api/packages", StatusCode::OK),
        ("/", StatusCode::OK),
    ] {
        let response = app
            .clone()
            .oneshot(Request::builder().uri(url).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        assert!(response.headers().contains_key("content-security-policy"));
    }
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/inspect")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"files":[{"path":"draft.flow.yaml","content":"invalid"}]}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["valid"], false);
    assert_eq!(value["files"][0]["content"], "invalid");
    assert_eq!(std::fs::read_dir(&config.temp).unwrap().count(), 0);
    for body in [
        r#"{"files":[{"path":"/etc/passwd","content":"x"}]}"#,
        r#"invalid json"#,
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/inspect")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        assert!(serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["error"].is_string());
        assert!(!String::from_utf8_lossy(&bytes).contains(dir.path().to_str().unwrap()));
    }
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/upload")
                .header("content-type", "application/octet-stream")
                .header("content-length", (65 * 1024 * 1024).to_string())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    assert!(serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["error"].is_string());
    std::fs::create_dir_all(config.packages.join("sample")).unwrap();
    std::fs::write(config.packages.join("sample/draft.flow.yaml"), "bad").unwrap();
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/packages")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let id = value["packages"][0]["id"].as_str().unwrap();
    assert_eq!(id.len(), 64);
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/packages/{id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}
