use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

#[derive(Debug)]
pub(crate) struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: &'static str,
}

pub(crate) type Result<T> = std::result::Result<T, ApiError>;

impl ApiError {
    pub(crate) const fn new(status: StatusCode, code: &'static str, message: &'static str) -> Self {
        Self {
            status,
            code,
            message,
        }
    }

    pub(crate) const fn storage() -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "STORAGE_UNAVAILABLE",
            "Local state is unavailable. Check local storage and try again.",
        )
    }

    pub(crate) const fn invalid_request() -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            "INVALID_REQUEST",
            "The request is invalid.",
        )
    }

    pub(crate) const fn session_required() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "SESSION_REQUIRED",
            "Sign in to continue.",
        )
    }

    pub(crate) const fn flow_not_found() -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            "FLOW_NOT_FOUND",
            "This flow is unavailable.",
        )
    }

    pub(crate) const fn run_not_found() -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            "RUN_NOT_FOUND",
            "This run is unavailable.",
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({ "error": { "code": self.code, "message": self.message } })),
        )
            .into_response()
    }
}
