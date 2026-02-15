use std::sync::Arc;

use axum::extract::{Path, State};
use axum::Json;
use serde_json::{json, Value as JsonValue};
use sqlx::PgPool;
use tokio::sync::Mutex;

use crate::api_error::ApiError;
use crate::manifest::quote_ident;
use crate::Runtime;
use rootcx_shared_types::{AppManifest, InstalledApp, OsStatus};

pub type SharedRuntime = Arc<Mutex<Runtime>>;

// ── Helpers ─────────────────────────────────────────

fn require_pool(runtime: &Runtime) -> Result<PgPool, ApiError> {
    runtime.pool().cloned().ok_or(ApiError::NotReady)
}

// ── Health ──────────────────────────────────────────

pub async fn health() -> Json<JsonValue> {
    Json(json!({ "status": "ok" }))
}

// ── Status ──────────────────────────────────────────

pub async fn get_status(State(rt): State<SharedRuntime>) -> Result<Json<OsStatus>, ApiError> {
    let runtime = rt.lock().await;
    Ok(Json(runtime.status().await))
}

// ── Apps ────────────────────────────────────────────

pub async fn install_app(
    State(rt): State<SharedRuntime>,
    Json(manifest): Json<AppManifest>,
) -> Result<Json<JsonValue>, ApiError> {
    let runtime = rt.lock().await;
    let pool = require_pool(&runtime)?;
    crate::install_app(&pool, &manifest).await?;
    Ok(Json(json!({
        "message": format!("app '{}' installed successfully", manifest.app_id)
    })))
}

pub async fn list_apps(State(rt): State<SharedRuntime>) -> Result<Json<Vec<InstalledApp>>, ApiError> {
    let runtime = rt.lock().await;
    let pool = require_pool(&runtime)?;

    let rows = sqlx::query_as::<_, (String, String, String, String, Option<sqlx::types::JsonValue>)>(
        r#"
        SELECT id, name, version, status, manifest
        FROM rootcx_system.apps
        ORDER BY name
        "#,
    )
    .fetch_all(&pool)
    .await?;

    let apps: Vec<InstalledApp> = rows
        .into_iter()
        .map(|(id, name, version, status, manifest)| {
            let entities = manifest
                .and_then(|m| {
                    m.get("dataContract")
                        .and_then(|dc| dc.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|e| {
                                    e.get("entityName").and_then(|n| n.as_str()).map(String::from)
                                })
                                .collect::<Vec<_>>()
                        })
                })
                .unwrap_or_default();

            InstalledApp {
                id,
                name,
                version,
                status,
                entities,
            }
        })
        .collect();

    Ok(Json(apps))
}

pub async fn uninstall_app(
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
) -> Result<Json<JsonValue>, ApiError> {
    let runtime = rt.lock().await;
    let pool = require_pool(&runtime)?;
    crate::uninstall_app(&pool, &app_id).await?;
    Ok(Json(json!({ "message": format!("app '{}' uninstalled", app_id) })))
}

// ── Collections CRUD ────────────────────────────────

pub async fn list_records(
    State(rt): State<SharedRuntime>,
    Path((app_id, entity)): Path<(String, String)>,
) -> Result<Json<Vec<JsonValue>>, ApiError> {
    let runtime = rt.lock().await;
    let pool = require_pool(&runtime)?;

    let table = format!("{}.{}", quote_ident(&app_id), quote_ident(&entity));
    let query = format!("SELECT to_jsonb(t.*) AS row FROM {} t ORDER BY created_at DESC", table);

    let rows: Vec<(JsonValue,)> = sqlx::query_as(&query)
        .fetch_all(&pool)
        .await?;

    Ok(Json(rows.into_iter().map(|(r,)| r).collect()))
}

pub async fn create_record(
    State(rt): State<SharedRuntime>,
    Path((app_id, entity)): Path<(String, String)>,
    Json(body): Json<JsonValue>,
) -> Result<(axum::http::StatusCode, Json<JsonValue>), ApiError> {
    let runtime = rt.lock().await;
    let pool = require_pool(&runtime)?;

    let obj = body.as_object().ok_or_else(|| ApiError::BadRequest("body must be a JSON object".into()))?;

    if obj.is_empty() {
        return Err(ApiError::BadRequest("body must not be empty".into()));
    }

    let table = format!("{}.{}", quote_ident(&app_id), quote_ident(&entity));

    let mut col_names = Vec::new();
    let mut placeholders = Vec::new();
    let mut values: Vec<JsonValue> = Vec::new();

    for (i, (key, val)) in obj.iter().enumerate() {
        col_names.push(quote_ident(key));
        placeholders.push(format!("${}", i + 1));
        values.push(val.clone());
    }

    let query = format!(
        "INSERT INTO {} ({}) VALUES ({}) RETURNING to_jsonb({}.*) AS row",
        table,
        col_names.join(", "),
        placeholders.join(", "),
        table,
    );

    // Build the query with bindings
    let mut q = sqlx::query_as::<_, (JsonValue,)>(&query);
    for val in &values {
        q = bind_json_value(q, val);
    }

    let (row,) = q
        .fetch_one(&pool)
        .await?;

    Ok((axum::http::StatusCode::CREATED, Json(row)))
}

pub async fn get_record(
    State(rt): State<SharedRuntime>,
    Path((app_id, entity, id)): Path<(String, String, String)>,
) -> Result<Json<JsonValue>, ApiError> {
    let runtime = rt.lock().await;
    let pool = require_pool(&runtime)?;

    let table = format!("{}.{}", quote_ident(&app_id), quote_ident(&entity));
    let query = format!("SELECT to_jsonb(t.*) AS row FROM {} t WHERE t.id = $1", table);

    let result: Option<(JsonValue,)> = sqlx::query_as(&query)
        .bind(&id)
        .fetch_optional(&pool)
        .await?;

    match result {
        Some((row,)) => Ok(Json(row)),
        None => Err(ApiError::NotFound(format!("record '{}' not found", id))),
    }
}

pub async fn update_record(
    State(rt): State<SharedRuntime>,
    Path((app_id, entity, id)): Path<(String, String, String)>,
    Json(body): Json<JsonValue>,
) -> Result<Json<JsonValue>, ApiError> {
    let runtime = rt.lock().await;
    let pool = require_pool(&runtime)?;

    let obj = body.as_object().ok_or_else(|| ApiError::BadRequest("body must be a JSON object".into()))?;

    if obj.is_empty() {
        return Err(ApiError::BadRequest("body must not be empty".into()));
    }

    let table = format!("{}.{}", quote_ident(&app_id), quote_ident(&entity));

    let mut set_clauses = Vec::new();
    let mut values: Vec<JsonValue> = Vec::new();

    for (i, (key, val)) in obj.iter().enumerate() {
        set_clauses.push(format!("{} = ${}", quote_ident(key), i + 1));
        values.push(val.clone());
    }

    // Always update the updated_at timestamp
    set_clauses.push("\"updated_at\" = now()".to_string());

    let id_placeholder = format!("${}", values.len() + 1);
    let query = format!(
        "UPDATE {} SET {} WHERE id = {} RETURNING to_jsonb({} .*) AS row",
        table,
        set_clauses.join(", "),
        id_placeholder,
        table,
    );

    let mut q = sqlx::query_as::<_, (JsonValue,)>(&query);
    for val in &values {
        q = bind_json_value(q, val);
    }
    q = q.bind(&id);

    let result = q
        .fetch_optional(&pool)
        .await?;

    match result {
        Some((row,)) => Ok(Json(row)),
        None => Err(ApiError::NotFound(format!("record '{}' not found", id))),
    }
}

pub async fn delete_record(
    State(rt): State<SharedRuntime>,
    Path((app_id, entity, id)): Path<(String, String, String)>,
) -> Result<Json<JsonValue>, ApiError> {
    let runtime = rt.lock().await;
    let pool = require_pool(&runtime)?;

    let table = format!("{}.{}", quote_ident(&app_id), quote_ident(&entity));
    let query = format!("DELETE FROM {} WHERE id = $1", table);

    let result: sqlx::postgres::PgQueryResult = sqlx::query(&query)
        .bind(&id)
        .execute(&pool)
        .await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound(format!("record '{}' not found", id)));
    }

    Ok(Json(json!({ "message": format!("record '{}' deleted", id) })))
}

// ── Bind helper ─────────────────────────────────────

/// Bind a serde_json::Value to a sqlx query, choosing the right Postgres type.
fn bind_json_value<'q>(
    q: sqlx::query::QueryAs<'q, sqlx::Postgres, (JsonValue,), sqlx::postgres::PgArguments>,
    val: &'q JsonValue,
) -> sqlx::query::QueryAs<'q, sqlx::Postgres, (JsonValue,), sqlx::postgres::PgArguments> {
    match val {
        JsonValue::Null => q.bind(None::<String>),
        JsonValue::Bool(b) => q.bind(b),
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                q.bind(i)
            } else if let Some(f) = n.as_f64() {
                q.bind(f)
            } else {
                q.bind(n.to_string())
            }
        }
        JsonValue::String(s) => q.bind(s.as_str()),
        _ => q.bind(val),
    }
}
