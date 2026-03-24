use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

#[derive(Debug)]
pub enum ApiError {
    NotFound(String),
    BadRequest(String),
    Unauthorized(String),
    Forbidden(String),
    NotReady,
    Internal(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            Self::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            Self::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, msg),
            Self::Forbidden(msg) => (StatusCode::FORBIDDEN, msg),
            Self::NotReady => (StatusCode::SERVICE_UNAVAILABLE, "runtime not ready".into()),
            Self::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };
        (status, axum::Json(json!({ "error": message }))).into_response()
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        tracing::error!("database error: {e}");
        Self::Internal("internal database error".into())
    }
}

impl From<crate::RuntimeError> for ApiError {
    fn from(e: crate::RuntimeError) -> Self {
        match &e {
            // Worker/Job/IPC errors are user-facing (e.g., "no worker for app 'x'")
            crate::RuntimeError::Worker(_) | crate::RuntimeError::Job(_) | crate::RuntimeError::Ipc(_) => {
                Self::Internal(e.to_string())
            }
            crate::RuntimeError::Migration(_) => Self::BadRequest(e.to_string()),
            _ => {
                tracing::error!("runtime error: {e}");
                Self::Internal("internal error".into())
            }
        }
    }
}
