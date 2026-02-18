use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Multipart, Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde_json::{json, Value as JsonValue};
use sqlx::types::Uuid;
use tokio::sync::Mutex;

use crate::api_error::ApiError;
use crate::manifest::quote_ident;
use crate::worker_manager::WorkerManager;
use crate::Runtime;
use rootcx_shared_types::{AppManifest, InstalledApp, OsStatus};

pub type SharedRuntime = Arc<Mutex<Runtime>>;

pub(crate) async fn pool(rt: &SharedRuntime) -> Result<sqlx::PgPool, ApiError> {
    rt.lock().await.pool().cloned().ok_or(ApiError::NotReady)
}

async fn wm(rt: &SharedRuntime) -> Result<Arc<WorkerManager>, ApiError> {
    rt.lock().await.worker_manager().cloned().ok_or(ApiError::NotReady)
}

fn parse_uuid(id: &str) -> Result<Uuid, ApiError> {
    id.parse::<Uuid>().map_err(|_| ApiError::BadRequest(format!("invalid UUID: '{id}'")))
}

fn table(app_id: &str, entity: &str) -> String {
    format!("{}.{}", quote_ident(app_id), quote_ident(entity))
}

fn require_object(body: &JsonValue) -> Result<&serde_json::Map<String, JsonValue>, ApiError> {
    let obj = body.as_object().ok_or_else(|| ApiError::BadRequest("body must be a JSON object".into()))?;
    if obj.is_empty() { return Err(ApiError::BadRequest("body must not be empty".into())); }
    Ok(obj)
}

// ── Core routes ───────────────────────────────────────────────────

pub async fn health() -> Json<JsonValue> { Json(json!({ "status": "ok" })) }

pub async fn get_status(State(rt): State<SharedRuntime>) -> Result<Json<OsStatus>, ApiError> {
    Ok(Json(rt.lock().await.status().await))
}

pub async fn install_app(State(rt): State<SharedRuntime>, Json(manifest): Json<AppManifest>) -> Result<Json<JsonValue>, ApiError> {
    let g = rt.lock().await;
    let pool = g.pool().cloned().ok_or(ApiError::NotReady)?;
    crate::manifest::install_app(&pool, &manifest, g.extensions()).await?;
    Ok(Json(json!({ "message": format!("app '{}' installed", manifest.app_id) })))
}

pub async fn list_apps(State(rt): State<SharedRuntime>) -> Result<Json<Vec<InstalledApp>>, ApiError> {
    let pool = pool(&rt).await?;
    let rows = sqlx::query_as::<_, (String, String, String, String, Option<sqlx::types::JsonValue>)>(
        "SELECT id, name, version, status, manifest FROM rootcx_system.apps ORDER BY name",
    ).fetch_all(&pool).await?;

    Ok(Json(rows.into_iter().map(|(id, name, version, status, manifest)| {
        let entities = manifest
            .and_then(|m| m.get("dataContract")?.as_array().map(|a|
                a.iter().filter_map(|e| e.get("entityName")?.as_str().map(String::from)).collect()
            )).unwrap_or_default();
        InstalledApp { id, name, version, status, entities }
    }).collect()))
}

pub async fn uninstall_app(State(rt): State<SharedRuntime>, Path(app_id): Path<String>) -> Result<Json<JsonValue>, ApiError> {
    if let Ok(w) = wm(&rt).await { let _ = w.stop_app(&app_id).await; }
    let pool = pool(&rt).await?;
    crate::manifest::uninstall_app(&pool, &app_id).await?;
    Ok(Json(json!({ "message": format!("app '{}' uninstalled", app_id) })))
}

// ── Collection CRUD ───────────────────────────────────────────────

pub async fn list_records(State(rt): State<SharedRuntime>, Path((app_id, entity)): Path<(String, String)>) -> Result<Json<Vec<JsonValue>>, ApiError> {
    let pool = pool(&rt).await?;
    let q = format!("SELECT to_jsonb(t.*) AS row FROM {} t ORDER BY created_at DESC", table(&app_id, &entity));
    let rows: Vec<(JsonValue,)> = sqlx::query_as(&q).fetch_all(&pool).await?;
    Ok(Json(rows.into_iter().map(|(r,)| r).collect()))
}

pub async fn create_record(State(rt): State<SharedRuntime>, Path((app_id, entity)): Path<(String, String)>, Json(body): Json<JsonValue>) -> Result<(StatusCode, Json<JsonValue>), ApiError> {
    let pool = pool(&rt).await?;
    let obj = require_object(&body)?;
    let tbl = table(&app_id, &entity);
    let values: Vec<_> = obj.values().cloned().collect();
    let (cols, phs): (Vec<_>, Vec<_>) = obj.keys().enumerate().map(|(i, k)| (quote_ident(k), format!("${}", i + 1))).unzip();
    let q = format!("INSERT INTO {} ({}) VALUES ({}) RETURNING to_jsonb({}.*) AS row", tbl, cols.join(", "), phs.join(", "), tbl);
    let mut query = sqlx::query_as::<_, (JsonValue,)>(&q);
    for v in &values { query = bind_json_value(query, v); }
    let (row,) = query.fetch_one(&pool).await?;
    Ok((StatusCode::CREATED, Json(row)))
}

pub async fn get_record(State(rt): State<SharedRuntime>, Path((app_id, entity, id)): Path<(String, String, String)>) -> Result<Json<JsonValue>, ApiError> {
    let (uuid, pool) = (parse_uuid(&id)?, pool(&rt).await?);
    let q = format!("SELECT to_jsonb(t.*) AS row FROM {} t WHERE t.id = $1", table(&app_id, &entity));
    sqlx::query_as::<_, (JsonValue,)>(&q).bind(uuid).fetch_optional(&pool).await?
        .map(|(r,)| Json(r)).ok_or_else(|| ApiError::NotFound(format!("record '{id}' not found")))
}

pub async fn update_record(State(rt): State<SharedRuntime>, Path((app_id, entity, id)): Path<(String, String, String)>, Json(body): Json<JsonValue>) -> Result<Json<JsonValue>, ApiError> {
    let (uuid, pool) = (parse_uuid(&id)?, pool(&rt).await?);
    let obj = require_object(&body)?;
    let tbl = table(&app_id, &entity);
    let (sets, vals): (Vec<_>, Vec<_>) = obj.iter().enumerate().map(|(i, (k, v))| (format!("{} = ${}", quote_ident(k), i + 1), v.clone())).unzip();
    let q = format!("UPDATE {} SET {}, \"updated_at\" = now() WHERE id = ${} RETURNING to_jsonb({} .*) AS row", tbl, sets.join(", "), vals.len() + 1, tbl);
    let mut query = sqlx::query_as::<_, (JsonValue,)>(&q);
    for v in &vals { query = bind_json_value(query, v); }
    query = query.bind(uuid);
    query.fetch_optional(&pool).await?.map(|(r,)| Json(r)).ok_or_else(|| ApiError::NotFound(format!("record '{id}' not found")))
}

pub async fn delete_record(State(rt): State<SharedRuntime>, Path((app_id, entity, id)): Path<(String, String, String)>) -> Result<Json<JsonValue>, ApiError> {
    let (uuid, pool) = (parse_uuid(&id)?, pool(&rt).await?);
    let r = sqlx::query(&format!("DELETE FROM {} WHERE id = $1", table(&app_id, &entity))).bind(uuid).execute(&pool).await?;
    if r.rows_affected() == 0 { return Err(ApiError::NotFound(format!("record '{id}' not found"))); }
    Ok(Json(json!({ "message": format!("record '{id}' deleted") })))
}

// ── Workers ───────────────────────────────────────────────────────

pub async fn start_worker(State(rt): State<SharedRuntime>, Path(app_id): Path<String>) -> Result<Json<JsonValue>, ApiError> {
    let (pool, secrets, w) = {
        let g = rt.lock().await;
        (
            g.pool().cloned().ok_or(ApiError::NotReady)?,
            g.secret_manager().cloned().ok_or(ApiError::NotReady)?,
            g.worker_manager().cloned().ok_or(ApiError::NotReady)?,
        )
    };
    w.start_app(&pool, &secrets, &app_id).await?;
    Ok(Json(json!({ "message": format!("worker '{}' started", app_id) })))
}

pub async fn stop_worker(State(rt): State<SharedRuntime>, Path(app_id): Path<String>) -> Result<Json<JsonValue>, ApiError> {
    wm(&rt).await?.stop_app(&app_id).await?;
    Ok(Json(json!({ "message": format!("worker '{}' stopped", app_id) })))
}

pub async fn worker_status(State(rt): State<SharedRuntime>, Path(app_id): Path<String>) -> Result<Json<JsonValue>, ApiError> {
    let s = wm(&rt).await?.worker_status(&app_id).await?;
    Ok(Json(json!({ "app_id": app_id, "status": s })))
}

pub async fn all_worker_statuses(State(rt): State<SharedRuntime>) -> Result<Json<JsonValue>, ApiError> {
    Ok(Json(json!({ "workers": wm(&rt).await?.all_statuses().await })))
}

// ── RPC proxy ─────────────────────────────────────────────────────

pub async fn rpc_proxy(State(rt): State<SharedRuntime>, Path(app_id): Path<String>, Json(body): Json<JsonValue>) -> Result<Json<JsonValue>, ApiError> {
    let method = body.get("method").and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::BadRequest("missing 'method'".into()))?.to_string();
    let params = body.get("params").cloned().unwrap_or(json!({}));
    let id = body.get("id").and_then(|v| v.as_str()).filter(|s| !s.is_empty())
        .map(String::from).unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    Ok(Json(wm(&rt).await?.rpc(&app_id, id, method, params).await?))
}

// ── Secrets ───────────────────────────────────────────────────────

pub async fn set_secret(State(rt): State<SharedRuntime>, Path(app_id): Path<String>, Json(body): Json<JsonValue>) -> Result<Json<JsonValue>, ApiError> {
    let key = body.get("key").and_then(|v| v.as_str()).ok_or_else(|| ApiError::BadRequest("missing 'key'".into()))?;
    let val = body.get("value").and_then(|v| v.as_str()).ok_or_else(|| ApiError::BadRequest("missing 'value'".into()))?;
    let (pool, sm) = {
        let g = rt.lock().await;
        (g.pool().cloned().ok_or(ApiError::NotReady)?, g.secret_manager().cloned().ok_or(ApiError::NotReady)?)
    };
    sm.set(&pool, &app_id, key, val).await?;
    Ok(Json(json!({ "message": format!("secret '{key}' set") })))
}

pub async fn delete_secret(State(rt): State<SharedRuntime>, Path((app_id, key)): Path<(String, String)>) -> Result<Json<JsonValue>, ApiError> {
    let (pool, sm) = {
        let g = rt.lock().await;
        (g.pool().cloned().ok_or(ApiError::NotReady)?, g.secret_manager().cloned().ok_or(ApiError::NotReady)?)
    };
    if sm.delete(&pool, &app_id, &key).await? {
        Ok(Json(json!({ "message": format!("secret '{key}' deleted") })))
    } else {
        Err(ApiError::NotFound(format!("secret '{key}' not found")))
    }
}

pub async fn list_secrets(State(rt): State<SharedRuntime>, Path(app_id): Path<String>) -> Result<Json<Vec<String>>, ApiError> {
    let (pool, sm) = {
        let g = rt.lock().await;
        (g.pool().cloned().ok_or(ApiError::NotReady)?, g.secret_manager().cloned().ok_or(ApiError::NotReady)?)
    };
    Ok(Json(sm.list_keys(&pool, &app_id).await?))
}

// ── Jobs ──────────────────────────────────────────────────────────

pub async fn enqueue_job(State(rt): State<SharedRuntime>, Path(app_id): Path<String>, Json(body): Json<JsonValue>) -> Result<(StatusCode, Json<JsonValue>), ApiError> {
    let pool = pool(&rt).await?;
    let payload = body.get("payload").cloned().unwrap_or(json!({}));
    let job_id = crate::jobs::enqueue(&pool, &app_id, payload, None).await?;
    if let Some(w) = rt.lock().await.scheduler_wake() { w.notify_one(); }
    Ok((StatusCode::CREATED, Json(json!({ "job_id": job_id }))))
}

pub async fn get_job(State(rt): State<SharedRuntime>, Path((_app_id, job_id)): Path<(String, String)>) -> Result<Json<JsonValue>, ApiError> {
    let pool = pool(&rt).await?;
    let job = crate::jobs::get(&pool, parse_uuid(&job_id)?).await?
        .ok_or_else(|| ApiError::NotFound(format!("job '{job_id}' not found")))?;
    Ok(Json(serde_json::to_value(job).unwrap_or(json!({}))))
}

pub async fn list_jobs(State(rt): State<SharedRuntime>, Path(app_id): Path<String>, Query(p): Query<HashMap<String, String>>) -> Result<Json<Vec<JsonValue>>, ApiError> {
    let pool = pool(&rt).await?;
    let limit: i64 = p.get("limit").and_then(|v| v.parse().ok()).unwrap_or(100).min(1000);
    let jobs = crate::jobs::list_for_app(&pool, &app_id, p.get("status").map(|s| s.as_str()), limit).await?;
    Ok(Json(jobs.into_iter().map(|j| serde_json::to_value(j).unwrap_or(json!({}))).collect()))
}

// ── File upload ───────────────────────────────────────────────────

pub async fn upload_file(State(rt): State<SharedRuntime>, Path(app_id): Path<String>, mut multipart: Multipart) -> Result<(StatusCode, Json<JsonValue>), ApiError> {
    let data_dir = rt.lock().await.data_dir().to_path_buf();
    let uploads_dir = data_dir.join("uploads").join(&app_id);
    tokio::fs::create_dir_all(&uploads_dir).await.map_err(|e| ApiError::Internal(e.to_string()))?;

    let field = multipart.next_field().await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?
        .ok_or_else(|| ApiError::BadRequest("no file field".into()))?;

    let name = field.file_name().unwrap_or("upload").to_string();
    let ext = std::path::Path::new(&name).extension().and_then(|e| e.to_str()).unwrap_or("bin");
    let file_id = uuid::Uuid::new_v4();
    let dest = uploads_dir.join(format!("{file_id}.{ext}"));

    let data = field.bytes().await.map_err(|e| ApiError::BadRequest(e.to_string()))?;
    tokio::fs::write(&dest, &data).await.map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok((StatusCode::CREATED, Json(json!({
        "file_id": file_id.to_string(),
        "original_name": name,
        "size": data.len(),
    }))))
}

// ── Helpers ───────────────────────────────────────────────────────

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
