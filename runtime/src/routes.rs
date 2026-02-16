use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde_json::{json, Value as JsonValue};
use sqlx::types::Uuid;
use tokio::sync::Mutex;

use crate::api_error::ApiError;
use crate::manifest::quote_ident;
use crate::Runtime;
use rootcx_shared_types::{AppManifest, InstalledApp, OsStatus};

pub type SharedRuntime = Arc<Mutex<Runtime>>;

/// Extract pool and immediately release the mutex lock.
async fn pool(rt: &SharedRuntime) -> Result<sqlx::PgPool, ApiError> {
    rt.lock().await.pool().cloned().ok_or(ApiError::NotReady)
}

fn parse_uuid(id: &str) -> Result<Uuid, ApiError> {
    id.parse::<Uuid>().map_err(|_| ApiError::BadRequest(format!("invalid UUID: '{id}'")))
}

fn table(app_id: &str, entity: &str) -> String {
    format!("{}.{}", quote_ident(app_id), quote_ident(entity))
}

fn require_object(body: &JsonValue) -> Result<&serde_json::Map<String, JsonValue>, ApiError> {
    let obj = body.as_object().ok_or_else(|| ApiError::BadRequest("body must be a JSON object".into()))?;
    if obj.is_empty() {
        return Err(ApiError::BadRequest("body must not be empty".into()));
    }
    Ok(obj)
}

pub async fn health() -> Json<JsonValue> {
    Json(json!({ "status": "ok" }))
}

pub async fn get_status(State(rt): State<SharedRuntime>) -> Result<Json<OsStatus>, ApiError> {
    Ok(Json(rt.lock().await.status().await))
}

pub async fn install_app(
    State(rt): State<SharedRuntime>,
    Json(manifest): Json<AppManifest>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = pool(&rt).await?;
    crate::manifest::install_app(&pool, &manifest).await?;
    Ok(Json(json!({ "message": format!("app '{}' installed successfully", manifest.app_id) })))
}

pub async fn list_apps(State(rt): State<SharedRuntime>) -> Result<Json<Vec<InstalledApp>>, ApiError> {
    let pool = pool(&rt).await?;

    let rows = sqlx::query_as::<_, (String, String, String, String, Option<sqlx::types::JsonValue>)>(
        "SELECT id, name, version, status, manifest FROM rootcx_system.apps ORDER BY name",
    )
    .fetch_all(&pool)
    .await?;

    let apps = rows
        .into_iter()
        .map(|(id, name, version, status, manifest)| {
            let entities = manifest
                .and_then(|m| {
                    m.get("dataContract")?.as_array().map(|arr| {
                        arr.iter()
                            .filter_map(|e| e.get("entityName")?.as_str().map(String::from))
                            .collect()
                    })
                })
                .unwrap_or_default();
            InstalledApp { id, name, version, status, entities }
        })
        .collect();

    Ok(Json(apps))
}

pub async fn uninstall_app(
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = pool(&rt).await?;
    crate::manifest::uninstall_app(&pool, &app_id).await?;
    Ok(Json(json!({ "message": format!("app '{}' uninstalled", app_id) })))
}

pub async fn list_records(
    State(rt): State<SharedRuntime>,
    Path((app_id, entity)): Path<(String, String)>,
) -> Result<Json<Vec<JsonValue>>, ApiError> {
    let pool = pool(&rt).await?;
    let query = format!("SELECT to_jsonb(t.*) AS row FROM {} t ORDER BY created_at DESC", table(&app_id, &entity));

    let rows: Vec<(JsonValue,)> = sqlx::query_as(&query).fetch_all(&pool).await?;
    Ok(Json(rows.into_iter().map(|(r,)| r).collect()))
}

pub async fn create_record(
    State(rt): State<SharedRuntime>,
    Path((app_id, entity)): Path<(String, String)>,
    Json(body): Json<JsonValue>,
) -> Result<(StatusCode, Json<JsonValue>), ApiError> {
    let pool = pool(&rt).await?;
    let obj = require_object(&body)?;
    let tbl = table(&app_id, &entity);

    let values: Vec<_> = obj.values().cloned().collect();
    let (cols, placeholders): (Vec<_>, Vec<_>) = obj
        .keys()
        .enumerate()
        .map(|(i, k)| (quote_ident(k), format!("${}", i + 1)))
        .unzip();

    let query = format!(
        "INSERT INTO {} ({}) VALUES ({}) RETURNING to_jsonb({}.*) AS row",
        tbl, cols.join(", "), placeholders.join(", "), tbl,
    );

    let mut q = sqlx::query_as::<_, (JsonValue,)>(&query);
    for val in &values { q = bind_json_value(q, val); }

    let (row,) = q.fetch_one(&pool).await?;
    Ok((StatusCode::CREATED, Json(row)))
}

pub async fn get_record(
    State(rt): State<SharedRuntime>,
    Path((app_id, entity, id)): Path<(String, String, String)>,
) -> Result<Json<JsonValue>, ApiError> {
    let (uuid, pool) = (parse_uuid(&id)?, pool(&rt).await?);
    let query = format!("SELECT to_jsonb(t.*) AS row FROM {} t WHERE t.id = $1", table(&app_id, &entity));

    sqlx::query_as::<_, (JsonValue,)>(&query)
        .bind(uuid)
        .fetch_optional(&pool)
        .await?
        .map(|(row,)| Json(row))
        .ok_or_else(|| ApiError::NotFound(format!("record '{id}' not found")))
}

pub async fn update_record(
    State(rt): State<SharedRuntime>,
    Path((app_id, entity, id)): Path<(String, String, String)>,
    Json(body): Json<JsonValue>,
) -> Result<Json<JsonValue>, ApiError> {
    let (uuid, pool) = (parse_uuid(&id)?, pool(&rt).await?);
    let obj = require_object(&body)?;
    let tbl = table(&app_id, &entity);

    let (set_clauses, values): (Vec<_>, Vec<_>) = obj
        .iter()
        .enumerate()
        .map(|(i, (k, v))| (format!("{} = ${}", quote_ident(k), i + 1), v.clone()))
        .unzip();

    let query = format!(
        "UPDATE {} SET {}, \"updated_at\" = now() WHERE id = ${} RETURNING to_jsonb({} .*) AS row",
        tbl, set_clauses.join(", "), values.len() + 1, tbl,
    );

    let mut q = sqlx::query_as::<_, (JsonValue,)>(&query);
    for val in &values { q = bind_json_value(q, val); }
    q = q.bind(uuid);

    q.fetch_optional(&pool)
        .await?
        .map(|(row,)| Json(row))
        .ok_or_else(|| ApiError::NotFound(format!("record '{id}' not found")))
}

pub async fn delete_record(
    State(rt): State<SharedRuntime>,
    Path((app_id, entity, id)): Path<(String, String, String)>,
) -> Result<Json<JsonValue>, ApiError> {
    let (uuid, pool) = (parse_uuid(&id)?, pool(&rt).await?);
    let query = format!("DELETE FROM {} WHERE id = $1", table(&app_id, &entity));

    let result = sqlx::query(&query).bind(uuid).execute(&pool).await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound(format!("record '{id}' not found")));
    }
    Ok(Json(json!({ "message": format!("record '{id}' deleted") })))
}

fn bind_json_value<'q>(
    q: sqlx::query::QueryAs<'q, sqlx::Postgres, (JsonValue,), sqlx::postgres::PgArguments>,
    val: &'q JsonValue,
) -> sqlx::query::QueryAs<'q, sqlx::Postgres, (JsonValue,), sqlx::postgres::PgArguments> {
    match val {
        JsonValue::Null => q.bind(None::<String>),
        JsonValue::Bool(b) => q.bind(b),
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() { q.bind(i) } else { q.bind(n.as_f64().unwrap_or(0.0)) }
        }
        JsonValue::String(s) => q.bind(s.as_str()),
        _ => q.bind(val),
    }
}
