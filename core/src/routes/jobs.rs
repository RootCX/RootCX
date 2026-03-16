use std::collections::HashMap;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde_json::{Value as JsonValue, json};

use super::{SharedRuntime, parse_uuid, pool};
use crate::api_error::ApiError;
use crate::auth::identity::Identity;

pub async fn enqueue_job(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
    Json(body): Json<JsonValue>,
) -> Result<(StatusCode, Json<JsonValue>), ApiError> {
    let pool = pool(&rt).await?;
    let payload = body.get("payload").cloned().unwrap_or(json!({}));
    let job_id = crate::jobs::enqueue(&pool, &app_id, payload, None, Some(identity.user_id)).await?;
    if let Some(w) = rt.lock().await.scheduler_wake() {
        w.notify_one();
    }
    Ok((StatusCode::CREATED, Json(json!({ "job_id": job_id }))))
}

pub async fn get_job(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((_app_id, job_id)): Path<(String, String)>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = pool(&rt).await?;
    let job = crate::jobs::get(&pool, parse_uuid(&job_id)?)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("job '{job_id}' not found")))?;
    Ok(Json(serde_json::to_value(job).unwrap_or(json!({}))))
}

pub async fn list_jobs(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
    Query(p): Query<HashMap<String, String>>,
) -> Result<Json<Vec<JsonValue>>, ApiError> {
    let pool = pool(&rt).await?;
    let limit: i64 = p.get("limit").and_then(|v| v.parse().ok()).unwrap_or(100).min(1000);
    let jobs = crate::jobs::list_for_app(&pool, &app_id, p.get("status").map(|s| s.as_str()), limit).await?;
    Ok(Json(jobs.into_iter().map(|j| serde_json::to_value(j).unwrap_or(json!({}))).collect()))
}
