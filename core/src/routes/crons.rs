use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde_json::{Value as JsonValue, json};
use uuid::Uuid;

use super::{SharedRuntime, pool};
use super::crud::validate_app_id;
use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::crons::{self, CreateCron};
use crate::extensions::rbac::policy::{resolve_permissions, has_permission};

async fn require_cron_perm(pool: &sqlx::PgPool, user_id: Uuid, app_id: &str, action: &str) -> Result<Vec<String>, ApiError> {
    let (_, perms) = resolve_permissions(pool, user_id).await?;
    if !has_permission(&perms, &format!("app:{app_id}:cron.{action}")) {
        return Err(ApiError::Forbidden(format!("missing app:{app_id}:cron.{action}")));
    }
    Ok(perms)
}

fn require_owner(perms: &[String], app_id: &str, row_owner: Option<Uuid>, caller: Uuid) -> Result<(), ApiError> {
    if let Some(owner) = row_owner {
        if owner != caller && !has_permission(perms, &format!("app:{app_id}:cron.manage_others")) {
            return Err(ApiError::Forbidden("not the cron owner".into()));
        }
    }
    Ok(())
}

pub async fn list_all_crons(
    identity: Identity,
    State(rt): State<SharedRuntime>,
) -> Result<Json<JsonValue>, ApiError> {
    let db = pool(&rt);
    let (_, perms) = resolve_permissions(&db, identity.user_id).await?;

    let app_ids: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM rootcx_system.apps WHERE status != 'system' ORDER BY name"
    ).fetch_all(&db).await?;

    let mut full_access = Vec::new();
    let mut own_only = Vec::new();
    for app_id in &app_ids {
        if !has_permission(&perms, &format!("app:{app_id}:cron.read")) { continue; }
        if has_permission(&perms, &format!("app:{app_id}:cron.manage_others")) {
            full_access.push(app_id.clone());
        } else {
            own_only.push(app_id.clone());
        }
    }

    let rows = crons::list_all(&db, &full_access, &own_only, identity.user_id).await?;
    Ok(Json(serde_json::to_value(rows).unwrap_or(json!([]))))
}

pub async fn create_cron(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
    Json(body): Json<JsonValue>,
) -> Result<(StatusCode, Json<JsonValue>), ApiError> {
    validate_app_id(&app_id)?;
    require_cron_perm(&pool(&rt), identity.user_id, &app_id, "write").await?;

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
        created_by: Some(identity.user_id),
    }).await?;

    rt.scheduler_wake().notify_one();
    Ok((StatusCode::CREATED, Json(serde_json::to_value(row).unwrap())))
}

pub async fn list_crons(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
) -> Result<Json<JsonValue>, ApiError> {
    validate_app_id(&app_id)?;
    let db = pool(&rt);
    let perms = require_cron_perm(&db, identity.user_id, &app_id, "read").await?;
    let rows = crons::list(&db, &app_id).await?;
    let rows = if has_permission(&perms, &format!("app:{app_id}:cron.manage_others")) {
        rows
    } else {
        rows.into_iter().filter(|r| r.created_by == Some(identity.user_id)).collect()
    };
    Ok(Json(serde_json::to_value(rows).unwrap_or(json!([]))))
}

pub async fn update_cron(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((app_id, id)): Path<(String, String)>,
    Json(body): Json<JsonValue>,
) -> Result<Json<JsonValue>, ApiError> {
    validate_app_id(&app_id)?;
    let cron_id = super::parse_uuid(&id)?;
    let db = pool(&rt);
    let perms = require_cron_perm(&db, identity.user_id, &app_id, "write").await?;
    let existing = crons::get(&db, &app_id, cron_id).await?;
    require_owner(&perms, &app_id, existing.created_by, identity.user_id)?;

    let schedule = body.get("schedule").and_then(|v| v.as_str());
    let payload = body.get("payload");
    let overlap = body.get("overlapPolicy").and_then(|v| v.as_str());
    let enabled = body.get("enabled").and_then(|v| v.as_bool());

    let row = crons::update(&db, &app_id, cron_id, schedule, payload, overlap, enabled).await?;
    Ok(Json(serde_json::to_value(row).unwrap()))
}

pub async fn delete_cron(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((app_id, id)): Path<(String, String)>,
) -> Result<Json<JsonValue>, ApiError> {
    validate_app_id(&app_id)?;
    let cron_id = super::parse_uuid(&id)?;
    let db = pool(&rt);
    let perms = require_cron_perm(&db, identity.user_id, &app_id, "write").await?;
    let existing = crons::get(&db, &app_id, cron_id).await?;
    require_owner(&perms, &app_id, existing.created_by, identity.user_id)?;
    crons::delete(&db, &app_id, cron_id).await?;
    Ok(Json(json!({ "message": "cron deleted" })))
}

pub async fn list_cron_runs(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((app_id, id)): Path<(String, String)>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<JsonValue>, ApiError> {
    validate_app_id(&app_id)?;
    let cron_id = super::parse_uuid(&id)?;
    let limit = params.get("limit").and_then(|v| v.parse::<i64>().ok()).unwrap_or(50).min(500);
    let db = pool(&rt);
    let perms = require_cron_perm(&db, identity.user_id, &app_id, "read").await?;
    let existing = crons::get(&db, &app_id, cron_id).await?;
    require_owner(&perms, &app_id, existing.created_by, identity.user_id)?;
    let runs = crons::list_runs(&db, &app_id, cron_id, limit).await?;
    Ok(Json(serde_json::to_value(runs).unwrap_or(json!([]))))
}

pub async fn trigger_cron(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((app_id, id)): Path<(String, String)>,
) -> Result<Json<JsonValue>, ApiError> {
    validate_app_id(&app_id)?;
    let cron_id = super::parse_uuid(&id)?;
    let db = pool(&rt);
    let perms = require_cron_perm(&db, identity.user_id, &app_id, "trigger").await?;
    let existing = crons::get(&db, &app_id, cron_id).await?;
    require_owner(&perms, &app_id, existing.created_by, identity.user_id)?;
    let msg_id = crons::trigger(&db, &app_id, cron_id).await?;
    rt.scheduler_wake().notify_one();
    Ok(Json(json!({ "msgId": msg_id })))
}
