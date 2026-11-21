use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{ensure, Context};
use axum::body::Bytes;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{DefaultBodyLimit, FromRequest, Path, Query, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{any, get, post, put};
use axum::{Json, Router};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use tower_http::services::ServeDir;
use zeroize::Zeroizing;

use crate::connections::ManualMode;
use crate::error::{ApiError, Result};
use crate::flows::is_sha;
use crate::runs::{backend, start_uncertain, Host, RunAction, StartRequest};

const JSON_LIMIT: usize = 512 * 1024;
const MAX_PASSPHRASE: usize = 1024;

#[derive(Clone)]
pub struct Origins {
    primary: SiteOrigin,
    alias: Option<SiteOrigin>,
}

#[derive(Clone)]
struct SiteOrigin {
    value: String,
    authority: String,
    scheme: String,
}

impl SiteOrigin {
    fn parse(value: &str) -> anyhow::Result<Self> {
        let url = url::Url::parse(value).context("invalid desktop origin")?;
        ensure!(
            matches!(url.scheme(), "http" | "https")
                && url.host_str().is_some()
                && url.origin().ascii_serialization() == value,
            "desktop origin must be an exact HTTP(S) origin without a path or credentials"
        );
        let (_, authority) = value
            .split_once("://")
            .context("missing desktop authority")?;
        Ok(Self {
            value: value.to_owned(),
            authority: authority.to_owned(),
            scheme: url.scheme().to_owned(),
        })
    }
}

impl Origins {
    pub fn new(primary: &str, alias: Option<&str>) -> anyhow::Result<Self> {
        let primary = SiteOrigin::parse(primary)?;
        let alias = alias.map(SiteOrigin::parse).transpose()?;
        if let Some(alias) = &alias {
            let primary_url = url::Url::parse(&primary.value)?;
            let alias_url = url::Url::parse(&alias.value)?;
            let loopback = |url: &url::Url| match url.host() {
                Some(url::Host::Domain(host)) => host == "localhost",
                Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
                Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
                None => false,
            };
            ensure!(
                loopback(&primary_url)
                    && loopback(&alias_url)
                    && primary_url.scheme() == alias_url.scheme()
                    && primary_url.port_or_known_default() == alias_url.port_or_known_default()
                    && primary.value != alias.value,
                "desktop origin alias must identify the same loopback listener"
            );
        }
        Ok(Self { primary, alias })
    }

    pub fn authority(&self) -> &str {
        &self.primary.authority
    }

    pub fn callback_url(&self) -> String {
        format!("{}/oauth/callback", self.primary.value)
    }

    fn admits(&self, request: &Request) -> bool {
        let mut hosts = request.headers().get_all(header::HOST).iter();
        let host = match hosts.next() {
            Some(host) => host.to_str().ok(),
            None => request
                .uri()
                .authority()
                .map(|authority| authority.as_str()),
        };
        let Some(host) = host else {
            return false;
        };
        if hosts.next().is_some() {
            return false;
        }
        let Some(site) = std::iter::once(&self.primary)
            .chain(self.alias.iter())
            .find(|site| site.authority.eq_ignore_ascii_case(host))
        else {
            return false;
        };
        if request
            .uri()
            .authority()
            .is_some_and(|authority| !authority.as_str().eq_ignore_ascii_case(host))
            || request
                .uri()
                .scheme_str()
                .is_some_and(|scheme| scheme != site.scheme)
        {
            return false;
        }
        let mut origins = request.headers().get_all(header::ORIGIN).iter();
        match origins.next() {
            Some(origin) => origin.as_bytes() == site.value.as_bytes() && origins.next().is_none(),
            None => true,
        }
    }
}

pub fn router(host: Arc<Host>, origins: Origins, static_dir: PathBuf) -> Router {
    Router::new()
        .route("/healthz", get(health))
        .route("/api/bootstrap", get(bootstrap))
        .route("/api/session", post(unlock).delete(sign_out))
        .route(
            "/oauth/callback/{issuer}",
            get(finish_oauth).head(|| async { StatusCode::METHOD_NOT_ALLOWED }),
        )
        .route("/api/flows", get(flows))
        .route(
            "/api/flows/import",
            post(import).layer(DefaultBodyLimit::max(
                trollm_uniffi::tar::MAX_BUNDLE_BYTES as usize,
            )),
        )
        .route("/api/flows/{sha}", get(flow))
        .route("/api/flows/{sha}/inspect", get(inspect))
        .route("/api/flows/{sha}/models/{slot}/download", post(download))
        .route("/api/flows/{sha}/models/{slot}/pause", post(pause_download))
        .route("/api/connections", get(connections))
        .route(
            "/api/connections/{name}",
            put(save_connection).delete(delete_connection),
        )
        .route("/api/connections/{name}/oauth/start", post(begin_oauth))
        .route("/api/runs", get(history).post(start_run))
        .route("/api/runs/{id}", get(observe_run))
        .route("/api/runs/{id}/action", post(run_action))
        .route("/api", any(api_not_found))
        .route("/api/{*path}", any(api_not_found))
        .method_not_allowed_fallback(|| async {
            ApiError::new(
                StatusCode::METHOD_NOT_ALLOWED,
                "METHOD_NOT_ALLOWED",
                "This method is unavailable.",
            )
        })
        .fallback_service(ServeDir::new(static_dir))
        .layer(DefaultBodyLimit::max(JSON_LIMIT))
        .layer(middleware::from_fn_with_state(
            (origins, host.clone()),
            browser_boundary,
        ))
        .with_state(host)
}

async fn browser_boundary(
    State((origins, host)): State<(Origins, Arc<Host>)>,
    request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();
    let api = path == "/api" || path.starts_with("/api/");
    let needs_session = api && !matches!(path, "/api/bootstrap" | "/api/session");
    let no_cache = api || path == "/oauth/callback" || path.starts_with("/oauth/callback/");
    let mut response = if !origins.admits(&request) {
        ApiError::new(
            StatusCode::FORBIDDEN,
            "ORIGIN_DENIED",
            "This request is outside the configured desktop origin.",
        )
        .into_response()
    } else if needs_session {
        match host.sessions.require(request.headers()) {
            Ok(()) => next.run(request).await,
            Err(error) => error.into_response(),
        }
    } else {
        next.run(request).await
    };
    let headers = response.headers_mut();
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    headers.insert("content-security-policy", HeaderValue::from_static(
        "default-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'",
    ));
    if no_cache {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    response
}

async fn api_not_found() -> ApiError {
    ApiError::new(
        StatusCode::NOT_FOUND,
        "NOT_FOUND",
        "This API endpoint is unavailable.",
    )
}

fn body<T: DeserializeOwned>(request: std::result::Result<Json<T>, JsonRejection>) -> Result<T> {
    request.map(|Json(value)| value).map_err(|rejection| {
        if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
            ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "REQUEST_TOO_LARGE",
                "The request exceeds its size limit.",
            )
        } else {
            ApiError::invalid_request()
        }
    })
}

fn path<T>(request: std::result::Result<Path<T>, PathRejection>) -> Result<T> {
    request
        .map(|Path(value)| value)
        .map_err(|_| ApiError::invalid_request())
}

fn query<T>(request: std::result::Result<Query<T>, QueryRejection>) -> Result<T> {
    request
        .map(|Query(value)| value)
        .map_err(|_| ApiError::invalid_request())
}

async fn blocking<T: Send + 'static>(
    task: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tokio::task::spawn_blocking(task)
        .await
        .map_err(|_| ApiError::storage())?
}

fn service_error(code: &'static str, message: &'static str) -> ApiError {
    ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, code, message)
}

async fn health() -> Json<Value> {
    Json(json!({ "status": "ok", "backend": backend() }))
}

async fn bootstrap(State(host): State<Arc<Host>>) -> Result<Json<Value>> {
    blocking(move || {
        Ok(Json(json!({
            "version": env!("CARGO_PKG_VERSION"),
            "backend": backend(),
            "needsSetup": !host.connections.initialized(),
        })))
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UnlockRequest {
    passphrase: String,
}

async fn unlock(
    State(host): State<Arc<Host>>,
    request: std::result::Result<Json<UnlockRequest>, JsonRejection>,
) -> Result<Json<Value>> {
    let passphrase = Zeroizing::new(body(request)?.passphrase);
    if passphrase.is_empty() || passphrase.len() > MAX_PASSPHRASE {
        return Err(ApiError::invalid_request());
    }
    let permit = host.login_slot.clone().try_acquire_owned().map_err(|_| {
        ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "UNLOCK_BUSY",
            "A sign-in attempt is already in progress.",
        )
    })?;
    host.sessions.admit_unlock()?;
    blocking(move || {
        let _permit = permit;
        host.connections.unlock(&passphrase).map_err(|_| {
            ApiError::new(
                StatusCode::UNAUTHORIZED,
                "UNLOCK_FAILED",
                "The vault could not be unlocked with this passphrase.",
            )
        })?;
        Ok(Json(json!({ "token": host.sessions.issue()? })))
    })
    .await
}

async fn sign_out(State(host): State<Arc<Host>>, headers: HeaderMap) -> Result<Json<Value>> {
    host.sessions.revoke(&headers)?;
    Ok(Json(json!({})))
}

async fn flows(State(host): State<Arc<Host>>) -> Result<Json<Value>> {
    blocking(move || Ok(Json(json!({ "flows": host.flows()? })))).await
}

async fn import(State(host): State<Arc<Host>>, request: Request) -> Result<Json<Value>> {
    if request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        != Some("application/x-tar")
    {
        return Err(ApiError::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "BUNDLE_MEDIA_TYPE",
            "Upload an application/x-tar flow bundle.",
        ));
    }
    let permit = host.import_slot.clone().try_acquire_owned().map_err(|_| {
        ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "IMPORT_BUSY",
            "Another import is in progress.",
        )
    })?;
    let upload = tokio::time::timeout(Duration::from_secs(30), Bytes::from_request(request, &()))
        .await
        .map_err(|_| {
            ApiError::new(
                StatusCode::REQUEST_TIMEOUT,
                "IMPORT_TIMEOUT",
                "The archive upload timed out.",
            )
        })?;
    let bytes = upload.map_err(|rejection| {
        if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
            ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "BUNDLE_TOO_LARGE",
                "The archive exceeds its size limit.",
            )
        } else {
            ApiError::invalid_request()
        }
    })?;
    blocking(move || {
        let _permit = permit;
        Ok(Json(json!({ "flow": host.import(bytes.to_vec())? })))
    })
    .await
}

async fn flow(
    State(host): State<Arc<Host>>,
    request: std::result::Result<Path<String>, PathRejection>,
) -> Result<Json<crate::runs::FlowDetails>> {
    let sha = path(request)?;
    blocking(move || Ok(Json(host.flow_details(&sha)?))).await
}

async fn inspect(
    State(host): State<Arc<Host>>,
    request: std::result::Result<Path<String>, PathRejection>,
) -> Result<Json<Value>> {
    let sha = path(request)?;
    blocking(move || {
        Ok(Json(
            host.load_flow(&sha)?
                .inspect()
                .map_err(|_| ApiError::storage())?,
        ))
    })
    .await
}

async fn download(
    State(host): State<Arc<Host>>,
    request: std::result::Result<Path<(String, String)>, PathRejection>,
) -> Result<Json<Value>> {
    model_action(host, path(request)?, false).await
}

async fn pause_download(
    State(host): State<Arc<Host>>,
    request: std::result::Result<Path<(String, String)>, PathRejection>,
) -> Result<Json<Value>> {
    model_action(host, path(request)?, true).await
}

async fn model_action(
    host: Arc<Host>,
    (sha, slot): (String, String),
    pause: bool,
) -> Result<Json<Value>> {
    blocking(move || {
        let setup = host
            .load_flow(&sha)?
            .info
            .setup
            .filter(|setup| setup.models.contains_key(&slot))
            .ok_or_else(|| {
                ApiError::new(
                    StatusCode::NOT_FOUND,
                    "MODEL_NOT_FOUND",
                    "This model slot is unavailable.",
                )
            })?;
        let statuses = if pause {
            host.models.pause(&setup, &slot)
        } else {
            host.models.start(&setup, &slot)
        }
        .map_err(|_| {
            service_error(
                "MODEL_ACTION_FAILED",
                "The model download could not be updated. Check storage and try again.",
            )
        })?;
        Ok(Json(json!({ "models": statuses })))
    })
    .await
}

async fn connections(State(host): State<Arc<Host>>) -> Result<Json<Value>> {
    blocking(move || {
        Ok(Json(json!({
            "connections": host.connections.list().map_err(|_| ApiError::storage())?
        })))
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConnectionRequest {
    url: String,
    mode: ManualMode,
    token: Option<String>,
}

async fn save_connection(
    State(host): State<Arc<Host>>,
    params: std::result::Result<Path<String>, PathRejection>,
    request: std::result::Result<Json<ConnectionRequest>, JsonRejection>,
) -> Result<Json<Value>> {
    let name = path(params)?;
    let request = body(request)?;
    let token = request.token.map(Zeroizing::new);
    blocking(move || {
        let connection = host.connections.save_manual(&name, &request.url, request.mode, token.as_ref().map(|token| token.as_str()))
            .map_err(|_| service_error("CONNECTION_REJECTED", "The connection could not be saved. Check its name, HTTPS endpoint and credentials."))?;
        Ok(Json(json!({ "connection": connection })))
    }).await
}

async fn delete_connection(
    State(host): State<Arc<Host>>,
    params: std::result::Result<Path<String>, PathRejection>,
) -> Result<Json<Value>> {
    let name = path(params)?;
    blocking(move || {
        host.connections.delete(&name).map_err(|_| {
            service_error(
                "CONNECTION_REJECTED",
                "The connection could not be removed.",
            )
        })?;
        Ok(Json(json!({})))
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OAuthRequest {
    url: String,
}

async fn begin_oauth(
    State(host): State<Arc<Host>>,
    params: std::result::Result<Path<String>, PathRejection>,
    request: std::result::Result<Json<OAuthRequest>, JsonRejection>,
) -> Result<Json<Value>> {
    let name = path(params)?;
    let request = body(request)?;
    blocking(move || {
        let url = host.connections.begin_oauth(&name, &request.url)
            .map_err(|_| service_error("OAUTH_UNAVAILABLE", "OAuth requires a publicly trusted HTTPS provider that accepts the desktop callback."))?;
        Ok(Json(json!({ "authorizationUrl": url })))
    }).await
}

#[derive(Deserialize)]
struct OAuthCallback {
    state: String,
    #[serde(default)]
    code: String,
    iss: Option<String>,
}

async fn finish_oauth(
    State(host): State<Arc<Host>>,
    issuer: std::result::Result<Path<String>, PathRejection>,
    request: std::result::Result<Query<OAuthCallback>, QueryRejection>,
) -> Redirect {
    if let (Ok(issuer), Ok(request)) = (path(issuer), query(request)) {
        if is_sha(&issuer) && !request.state.is_empty() && request.state.len() <= 512 {
            let connected = blocking(move || {
                let state = Zeroizing::new(request.state);
                let code = Zeroizing::new(request.code);
                let code = if code.len() <= 4096 {
                    code.as_str()
                } else {
                    ""
                };
                let iss =
                    request
                        .iss
                        .as_deref()
                        .map(|issuer| if issuer.len() <= 4096 { issuer } else { "" });
                Ok(host
                    .connections
                    .finish_oauth(&issuer, &state, code, iss)
                    .is_ok())
            })
            .await
            .unwrap_or(false);
            if connected {
                return Redirect::to("/?oauth=connected");
            }
        }
    }
    Redirect::to("/?oauth=failed")
}

async fn history(State(host): State<Arc<Host>>) -> Result<Json<Value>> {
    blocking(move || Ok(Json(json!({ "runs": host.history()? })))).await
}

async fn start_run(
    State(host): State<Arc<Host>>,
    request: std::result::Result<Json<StartRequest>, JsonRejection>,
) -> Result<Json<crate::runs::StartResponse>> {
    let request = body(request)?;
    // The task owns completion of the durable receipt even if the browser
    // loses its response or disconnects while the runtime is starting.
    tokio::task::spawn_blocking(move || host.start(request).map(Json))
        .await
        .map_err(|_| start_uncertain())?
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    #[serde(default)]
    after: u64,
}

async fn observe_run(
    State(host): State<Arc<Host>>,
    params: std::result::Result<Path<String>, PathRejection>,
    request: std::result::Result<Query<Cursor>, QueryRejection>,
) -> Result<Json<crate::runs::RunDetails>> {
    let id = path(params)?;
    let cursor = query(request)?;
    blocking(move || Ok(Json(host.observe(&id, cursor.after)?))).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunActionRequest {
    action: RunAction,
}

async fn run_action(
    State(host): State<Arc<Host>>,
    params: std::result::Result<Path<String>, PathRejection>,
    request: std::result::Result<Json<RunActionRequest>, JsonRejection>,
) -> Result<Json<Value>> {
    let id = path(params)?;
    let request = body(request)?;
    blocking(move || Ok(Json(json!({ "run": host.action(&id, request.action)? })))).await
}
