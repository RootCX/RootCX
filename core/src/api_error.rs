use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

#[derive(Debug)]
pub enum ApiError {
    NotFound(String),
    BadRequest(String),
    Unauthorized(String),
    Forbidden(String),
    Conflict(String),
    NotReady,
    /// A transient inability to serve, with a reason. Distinct from `NotReady`,
    /// which is boot state, and from `Internal`, which is a fault.
    Unavailable(String),
    Internal(String),
    /// A write an app's declared database rule rejected. Carries schema names
    /// only: PostgreSQL's DETAIL contains row values the caller may not read.
    RuleViolation { entity: String, rule: String },
    /// A write a unique index rejected; same disclosure boundary.
    UniqueViolation { entity: String, index: String },
}

/// Schemas the Core owns. Rule and uniqueness failures there are Core faults,
/// never an app's declared rule, and keep the opaque internal error.
const CORE_SCHEMAS: &[&str] = &["rootcx_system", "rootcx_ext", "pgmq", "cron", "public"];

fn app_rule_violation(db_err: &dyn sqlx::error::DatabaseError) -> Option<ApiError> {
    let pg = db_err.try_downcast_ref::<sqlx::postgres::PgDatabaseError>()?;
    let (schema, table, name) = (pg.schema()?, pg.table()?, pg.constraint()?);
    if CORE_SCHEMAS.contains(&schema) || schema.starts_with("pg_") {
        return None;
    }
    let (entity, name) = (table.to_string(), name.to_string());
    match pg.code() {
        "23514" => Some(ApiError::RuleViolation { entity, rule: name }),
        "23505" => Some(ApiError::UniqueViolation { entity, index: name }),
        _ => None,
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::RuleViolation { entity, rule } => return (
                StatusCode::UNPROCESSABLE_ENTITY,
                axum::Json(json!({ "error": "rule_violation", "entity": entity, "rule": rule })),
            ).into_response(),
            Self::UniqueViolation { entity, index } => return (
                StatusCode::CONFLICT,
                axum::Json(json!({ "error": "unique_violation", "entity": entity, "index": index })),
            ).into_response(),
            Self::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            Self::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            Self::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, msg),
            Self::Forbidden(msg) => (StatusCode::FORBIDDEN, msg),
            Self::Conflict(msg) => (StatusCode::CONFLICT, msg),
            Self::NotReady => (StatusCode::SERVICE_UNAVAILABLE, "runtime not ready".into()),
            Self::Unavailable(msg) => (StatusCode::SERVICE_UNAVAILABLE, msg),
            Self::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };
        // Every 5xx is logged HERE rather than at its ~100 construction sites.
        // The `From<sqlx::Error>` and `From<RuntimeError>` arms used to log while
        // hand-built `ApiError::Internal(format!(...))` did not, so the only copy
        // of most failures was the response body the caller then discarded — the
        // reason an every-action outage had to be reported by the customer.
        // One choke point cannot be forgotten by a new call site.
        if status.is_server_error() {
            tracing::error!(status = status.as_u16(), error = %message, "request failed");
        }
        (status, axum::Json(json!({ "error": message }))).into_response()
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        if let sqlx::Error::Database(ref db_err) = e {
            match db_err.code().as_deref() {
                // RLS denied the write (INSERT/UPDATE WITH CHECK): contract is 403, not 500.
                Some("42501") => return Self::Forbidden("write rejected by access policy".into()),
                Some("23503") => {
                    let detail = db_err.message();
                    return Self::Conflict(format!("foreign key constraint violated: {detail}"));
                }
                Some("23514" | "23505") => {
                    if let Some(error) = app_rule_violation(db_err.as_ref()) {
                        return error;
                    }
                }
                _ => {}
            }
        }
        tracing::error!("database error: {e}");
        Self::Internal("internal database error".into())
    }
}

impl From<crate::RuntimeError> for ApiError {
    fn from(e: crate::RuntimeError) -> Self {
        match &e {
            crate::RuntimeError::PermissionDenied(_) => Self::Forbidden(e.to_string()),
            // Worker/Job/IPC errors are user-facing (e.g., "no worker for app 'x'")
            crate::RuntimeError::Conflict(_) => Self::Conflict(e.to_string()),
            crate::RuntimeError::NotFound(_) => Self::NotFound(e.to_string()),
            crate::RuntimeError::Cron(_) | crate::RuntimeError::Invalid(_) => {
                Self::BadRequest(e.to_string())
            }
            crate::RuntimeError::Capacity(_) => Self::Unavailable(e.to_string()),
            crate::RuntimeError::Worker(_) | crate::RuntimeError::Job(_) | crate::RuntimeError::Ipc(_) => {
                Self::Internal(e.to_string())
            }
            _ => {
                tracing::error!("runtime error: {e}");
                Self::Internal("internal error".into())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;

    fn status_of(error: crate::RuntimeError) -> StatusCode {
        ApiError::from(error).into_response().status()
    }

    /// A rejected manifest must reach its author with the reason. These once
    /// travelled as `Schema(sqlx::Error::Protocol(..))`, which the catch-all arm
    /// turned into `500 internal error` — the validator's message was written,
    /// then discarded before the response. The pairing with a genuine database
    /// failure is the point: that one must stay opaque.
    /// Being at capacity is transient, not a fault. As a 500 it tells an operator
    /// to hunt a bug and tells a client never to retry; the caller's own next
    /// attempt, seconds later, is very likely to succeed. The pairing matters:
    /// a genuine worker failure must stay a 500.
    #[test]
    fn running_out_of_worker_slots_is_retryable_not_a_fault() {
        assert_eq!(
            status_of(crate::RuntimeError::Capacity("worker capacity reached".into())),
            StatusCode::SERVICE_UNAVAILABLE,
        );
        assert_eq!(
            status_of(crate::RuntimeError::Worker("spawn failed".into())),
            StatusCode::INTERNAL_SERVER_ERROR,
            "a real worker failure is still a fault",
        );
    }

    #[test]
    fn a_rejected_manifest_is_a_bad_request_not_an_internal_error() {
        assert_eq!(
            status_of(crate::RuntimeError::Invalid("field 'x' has unknown type".into())),
            StatusCode::BAD_REQUEST,
        );
        assert_eq!(
            status_of(crate::RuntimeError::Schema(sqlx::Error::PoolClosed)),
            StatusCode::INTERNAL_SERVER_ERROR,
            "a real schema failure stays opaque",
        );
    }
}
