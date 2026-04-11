use std::collections::HashMap;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde_json::{Value as JsonValue, json};

use super::{SharedRuntime, pool};
use crate::api_error::ApiError;
use crate::auth::identity::Identity;

pub async fn enqueue_job(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
    Json(body): Json<JsonValue>,
) -> Result<(StatusCode, Json<JsonValue>), ApiError> {
    let pool = pool(&rt);
    let payload = body.get("payload").cloned().unwrap_or(json!({}));
    let run_as = match body.get("user_id").and_then(|v| v.as_str()) {
        Some(uid) => {
            let target: uuid::Uuid = uid.parse().map_err(|_| ApiError::BadRequest("invalid user_id".into()))?;
            if target != identity.user_id {
                crate::extensions::rbac::policy::require_admin(&pool, identity.user_id).await?;
            }
            Some(target)
        }
        None => Some(identity.user_id),
    };
    let msg_id = crate::jobs::enqueue(&pool, &app_id, payload, run_as).await?;
    rt.scheduler_wake().notify_one();
    Ok((StatusCode::CREATED, Json(json!({ "msg_id": msg_id }))))
}

pub async fn list_jobs(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
    Query(p): Query<HashMap<String, String>>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = pool(&rt);
    let limit: i64 = p.get("limit").and_then(|v| v.parse().ok()).unwrap_or(100).min(1000);
    let archived = p.get("archived").map(|v| v == "true").unwrap_or(false);
    let jobs = if archived {
        crate::jobs::list_archived(&pool, &app_id, limit).await?
    } else {
        crate::jobs::list_for_app(&pool, &app_id, limit).await?
    };
    Ok(Json(serde_json::to_value(jobs).unwrap_or(json!([]))))
}
