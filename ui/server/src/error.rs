use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

/// An error that is shown to the client as `{"error": "..."}`.
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, message: impl Into<String>) -> ApiError {
        ApiError { status, message: message.into() }
    }

    /// A database failure. The details go to the log, never to the browser
    /// (messages can contain query text).
    pub fn db(e: impl std::fmt::Display) -> ApiError {
        tracing::warn!("database error: {e}");
        ApiError::new(StatusCode::BAD_GATEWAY, "the database request failed")
    }

    pub fn unauthorized() -> ApiError {
        ApiError::new(StatusCode::UNAUTHORIZED, "login required")
    }

    pub fn forbidden() -> ApiError {
        ApiError::new(StatusCode::FORBIDDEN, "this needs the admin role")
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.message }))).into_response()
    }
}
