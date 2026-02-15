use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// Unified error type for the HTTP API layer.
#[derive(Debug)]
pub enum ApiError {
    /// Entity/app not found.
    NotFound(String),
    /// Bad request (invalid input).
    BadRequest(String),
    /// Runtime not ready (database pool unavailable).
    NotReady,
    /// Internal runtime or database error.
    Internal(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ApiError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            ApiError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            ApiError::NotReady => (
                StatusCode::SERVICE_UNAVAILABLE,
                "runtime not ready — database pool unavailable".to_string(),
            ),
            ApiError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };

        let body = axum::Json(json!({ "error": message }));
        (status, body).into_response()
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        ApiError::Internal(e.to_string())
    }
}

impl From<crate::RuntimeError> for ApiError {
    fn from(e: crate::RuntimeError) -> Self {
        ApiError::Internal(e.to_string())
    }
}
