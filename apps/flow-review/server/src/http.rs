use crate::{
    import::{self, SourceFile, SourceRequest, MAX_BYTES},
    inspect::{self, Inspection},
};
use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Path as ApiPath, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{fs, path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::Semaphore;
use tower_http::{services::ServeDir, set_header::SetResponseHeaderLayer, timeout::TimeoutLayer};

#[derive(Clone)]
pub struct Config {
    pub packages: PathBuf,
    pub temp: PathBuf,
    pub static_dir: PathBuf,
}
#[derive(Clone)]
struct AppState {
    config: Config,
    imports: Arc<Semaphore>,
}
#[derive(Serialize)]
struct CatalogItem {
    id: String,
    name: String,
    kind: &'static str,
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Selection {
    entry_flow: Option<String>,
    task_registry: Option<String>,
}

type ApiResult<T> = Result<Json<T>, ApiError>;
struct ApiError(StatusCode, &'static str);
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error":self.1}))).into_response()
    }
}
fn bad() -> ApiError {
    ApiError(
        StatusCode::BAD_REQUEST,
        "Package is unreadable or violates the safe text package limits",
    )
}

fn catalog(config: &Config) -> anyhow::Result<Vec<(CatalogItem, PathBuf)>> {
    if !config.packages.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for (index, entry) in fs::read_dir(&config.packages)?.enumerate() {
        if index >= 1024 {
            anyhow::bail!("catalog limit exceeded");
        }
        let entry = entry?;
        let meta = fs::symlink_metadata(entry.path())?;
        if meta.file_type().is_symlink() {
            continue;
        }
        let kind = if meta.is_dir() {
            "source"
        } else if meta.is_file() && entry.path().extension().is_some_and(|e| e == "tar") {
            "bundle"
        } else {
            continue;
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        let id = trollm_core::plan::hex_sha256(name.as_bytes());
        out.push((CatalogItem { id, name, kind }, entry.path()));
        if out.len() > 1024 {
            anyhow::bail!("catalog limit exceeded");
        }
    }
    out.sort_by(|a, b| a.0.name.cmp(&b.0.name));
    Ok(out)
}

pub fn router(config: Config) -> Router {
    let assets = ServeDir::new(config.static_dir.clone()).append_index_html_on_directories(true);
    let state = AppState {
        config,
        imports: Arc::new(Semaphore::new(2)),
    };
    Router::new().route("/healthz", get(|| async { Json(json!({"status":"ok"})) }))
        .route("/api/packages", get(packages)).route("/api/packages/{id}", get(mapped))
        .route("/api/inspect", post(source)).route("/api/upload", post(upload))
        .fallback_service(assets)
        .layer(axum::middleware::from_fn_with_state(Arc::new(Semaphore::new(2)), admission))
        .layer(DefaultBodyLimit::max(MAX_BYTES))
        .layer(tower_http::limit::RequestBodyLimitLayer::new(MAX_BYTES))
        .layer(TimeoutLayer::with_status_code(StatusCode::REQUEST_TIMEOUT, Duration::from_secs(60)))
        .layer(axum::middleware::from_fn(json_errors))
        .layer(SetResponseHeaderLayer::if_not_present(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")))
        .layer(SetResponseHeaderLayer::if_not_present(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer")))
        .layer(SetResponseHeaderLayer::if_not_present(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'")))
        .with_state(state)
}
async fn packages(State(state): State<AppState>) -> ApiResult<serde_json::Value> {
    let config = state.config;
    let items = tokio::task::spawn_blocking(move || catalog(&config))
        .await
        .map_err(|_| bad())?
        .map_err(|_| bad())?;
    Ok(Json(
        json!({ "packages": items.into_iter().map(|(item,_)| item).collect::<Vec<_>>() }),
    ))
}

async fn inspect_job<F>(state: AppState, job: F) -> ApiResult<Inspection>
where
    F: FnOnce(&Config) -> anyhow::Result<(SourceRequest, String)> + Send + 'static,
{
    // Fail fast: bounded work and no unbounded queue of in-memory uploads.
    let permit = state.imports.try_acquire_owned().map_err(|_| {
        ApiError(
            StatusCode::TOO_MANY_REQUESTS,
            "Two inspections are already in progress; retry shortly",
        )
    })?;
    let config = state.config;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let (request, name) = job(&config)?;
        inspect::inspect(request, &config.temp, &name)
    })
    .await
    .map_err(|_| bad())?
    .map(Json)
    .map_err(|_| bad())
}
async fn mapped(
    State(state): State<AppState>,
    ApiPath(id): ApiPath<String>,
    Query(selection): Query<Selection>,
) -> ApiResult<Inspection> {
    inspect_job(state, move |config| {
        let (item, path) = catalog(config)?
            .into_iter()
            .find(|(item, _)| item.id == id)
            .ok_or_else(|| anyhow::anyhow!("unknown package"))?;
        let files = if item.kind == "source" {
            import::directory(&path)?
        } else {
            let meta = fs::symlink_metadata(&path)?;
            if meta.file_type().is_symlink() || !meta.is_file() || meta.len() > MAX_BYTES as u64 {
                anyhow::bail!("unsafe mapped archive");
            }
            use std::io::Read;
            let mut bytes = Vec::new();
            import::open_regular(&path)?
                .take((MAX_BYTES + 1) as u64)
                .read_to_end(&mut bytes)?;
            import::archive(&bytes)?
        };
        Ok((
            SourceRequest {
                files: files
                    .into_iter()
                    .map(|(path, content)| SourceFile { path, content })
                    .collect(),
                entry_flow: selection.entry_flow,
                task_registry: selection.task_registry,
            },
            item.name,
        ))
    })
    .await
}
async fn source(State(state): State<AppState>, body: Bytes) -> ApiResult<Inspection> {
    inspect_job(state, move |_| {
        Ok((serde_json::from_slice(&body)?, "Imported sources".into()))
    })
    .await
}
async fn upload(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Inspection> {
    if headers
        .get(header::CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        != Some("application/octet-stream")
    {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "Upload requires application/octet-stream uncompressed tar",
        ));
    }
    inspect_job(state, move |_| {
        Ok((
            SourceRequest {
                files: import::archive(&body)?
                    .into_iter()
                    .map(|(path, content)| SourceFile { path, content })
                    .collect(),
                ..Default::default()
            },
            "Imported bundle".into(),
        ))
    })
    .await
}

async fn json_errors(request: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let api = request.uri().path().starts_with("/api/");
    let response = next.run(request).await;
    if api
        && response.status().is_client_error()
        && response
            .headers()
            .get(header::CONTENT_TYPE)
            .is_none_or(|value| value != "application/json")
        && response.status() != StatusCode::PAYLOAD_TOO_LARGE
    {
        return ApiError(response.status(), "Invalid API request").into_response();
    }
    match response.status() {
        StatusCode::PAYLOAD_TOO_LARGE => ApiError(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Request exceeds 64 MiB limit",
        )
        .into_response(),
        StatusCode::REQUEST_TIMEOUT => {
            ApiError(StatusCode::REQUEST_TIMEOUT, "Inspection request timed out").into_response()
        }
        _ => response,
    }
}

async fn admission(
    State(slots): State<Arc<Semaphore>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let path = request.uri().path();
    if path == "/api/inspect" || path == "/api/upload" || path.starts_with("/api/packages/") {
        let Ok(_permit) = slots.try_acquire_owned() else {
            return ApiError(
                StatusCode::TOO_MANY_REQUESTS,
                "Two inspections are already in progress; retry shortly",
            )
            .into_response();
        };
        next.run(request).await
    } else {
        next.run(request).await
    }
}
