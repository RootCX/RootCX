use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde_json::{Value as JsonValue, json};

use super::{SharedRuntime, pool};
use super::crud::validate_app_id;
use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::crons::{self, CreateCron};

pub async fn create_cron(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
    Json(body): Json<JsonValue>,
) -> Result<(StatusCode, Json<JsonValue>), ApiError> {
    validate_app_id(&app_id)?;
    let name = body.get("name").and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::BadRequest("missing 'name'".into()))?;
    let schedule = body.get("schedule").and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::BadRequest("missing 'schedule'".into()))?;
    let timezone = body.get("timezone").and_then(|v| v.as_str()).map(String::from);
    let payload = body.get("payload").cloned().unwrap_or(json!({}));
    let overlap = body.get("overlapPolicy").and_then(|v| v.as_str()).unwrap_or("skip");

    let row = crons::create(&pool(&rt), &app_id, CreateCron {
        name: name.into(),
        schedule: schedule.into(),
        timezone,
        payload,
        overlap_policy: overlap.into(),
    }).await?;

    rt.scheduler_wake().notify_one();
    Ok((StatusCode::CREATED, Json(serde_json::to_value(row).unwrap())))
}

pub async fn list_crons(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
) -> Result<Json<JsonValue>, ApiError> {
    validate_app_id(&app_id)?;
    let rows = crons::list(&pool(&rt), &app_id).await?;
    Ok(Json(serde_json::to_value(rows).unwrap_or(json!([]))))
}

pub async fn update_cron(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((app_id, id)): Path<(String, String)>,
    Json(body): Json<JsonValue>,
) -> Result<Json<JsonValue>, ApiError> {
    validate_app_id(&app_id)?;
    let cron_id = super::parse_uuid(&id)?;
    let schedule = body.get("schedule").and_then(|v| v.as_str());
    let payload = body.get("payload");
    let overlap = body.get("overlapPolicy").and_then(|v| v.as_str());
    let enabled = body.get("enabled").and_then(|v| v.as_bool());

    let row = crons::update(&pool(&rt), &app_id, cron_id, schedule, payload, overlap, enabled).await?;
    Ok(Json(serde_json::to_value(row).unwrap()))
}

pub async fn delete_cron(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((app_id, id)): Path<(String, String)>,
) -> Result<Json<JsonValue>, ApiError> {
    validate_app_id(&app_id)?;
    let cron_id = super::parse_uuid(&id)?;
    crons::delete(&pool(&rt), &app_id, cron_id).await?;
    Ok(Json(json!({ "message": "cron deleted" })))
}

pub async fn trigger_cron(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((app_id, id)): Path<(String, String)>,
) -> Result<Json<JsonValue>, ApiError> {
    validate_app_id(&app_id)?;
    let cron_id = super::parse_uuid(&id)?;
    let msg_id = crons::trigger(&pool(&rt), &app_id, cron_id).await?;
    rt.scheduler_wake().notify_one();
    Ok(Json(json!({ "msgId": msg_id })))
}
