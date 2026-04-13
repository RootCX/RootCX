pub mod backend;
pub mod nonce;
#[cfg(test)]
#[path = "backend_test.rs"]
mod backend_test;

use async_trait::async_trait;
use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, Multipart, Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value as JsonValue};
use sqlx::PgPool;
use tracing::info;
use uuid::Uuid;

use crate::RuntimeError;
use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::routes::SharedRuntime;
use backend::{PostgresBackend, StorageBackend};

use super::RuntimeExtension;

const MAX_FILE_BYTES: usize = 64 * 1024 * 1024; // 64 MiB

async fn exec(pool: &PgPool, sql: &str) -> Result<(), RuntimeError> {
    sqlx::query(sql).execute(pool).await.map_err(RuntimeError::Schema)?;
    Ok(())
}

pub struct StorageExtension;

#[async_trait]
impl RuntimeExtension for StorageExtension {
    fn name(&self) -> &str {
        "storage"
    }

    async fn bootstrap(&self, pool: &PgPool) -> Result<(), RuntimeError> {
        info!("bootstrapping storage extension");

        exec(pool, r#"
            CREATE TABLE IF NOT EXISTS rootcx_system.files (
                id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                app_id       TEXT NOT NULL,
                name         TEXT NOT NULL,
                content_type TEXT NOT NULL DEFAULT 'application/octet-stream',
                size         BIGINT NOT NULL,
                content      BYTEA NOT NULL,
                uploaded_by  UUID,
                created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
            )
        "#).await?;

        exec(pool, "CREATE INDEX IF NOT EXISTS idx_files_app ON rootcx_system.files (app_id)").await?;
        exec(pool, "CREATE INDEX IF NOT EXISTS idx_files_created ON rootcx_system.files (created_at DESC)").await?;

        info!("storage extension ready");
        Ok(())
    }

    fn routes(&self) -> Option<Router<SharedRuntime>> {
        Some(
            Router::new()
                // Nonce-authenticated upload for workers (no Identity required)
                .route("/api/v1/storage/upload/{nonce}", post(upload_via_nonce).layer(DefaultBodyLimit::max(MAX_FILE_BYTES)))
                // JWT-authenticated upload for users/frontend — scoped by app_id
                .route("/api/v1/apps/{app_id}/storage/upload", post(upload_file).layer(DefaultBodyLimit::max(MAX_FILE_BYTES)))
                // JWT-authenticated download/delete — scoped by app_id
                .route("/api/v1/apps/{app_id}/storage/{file_id}", get(get_file).delete(delete_file))
        )
    }
}

fn backend() -> PostgresBackend {
    PostgresBackend
}

/// POST /api/v1/storage/upload/{nonce} — worker upload via single-use nonce.
/// No JWT required. The nonce proves the upload was authorized by Core via IPC.
async fn upload_via_nonce(
    State(rt): State<SharedRuntime>,
    Path(nonce_id): Path<String>,
    body: Bytes,
) -> Result<(StatusCode, Json<JsonValue>), ApiError> {
    let upload_nonce = rt.upload_nonces().lock().unwrap_or_else(|e| e.into_inner()).consume(&nonce_id)
        .ok_or_else(|| ApiError::NotFound("invalid or expired upload nonce".into()))?;

    if body.is_empty() {
        return Err(ApiError::BadRequest("empty file".into()));
    }
    if upload_nonce.max_size > 0 && body.len() > upload_nonce.max_size {
        return Err(ApiError::BadRequest(format!("file exceeds declared size ({} bytes)", upload_nonce.max_size)));
    }

    let pool = rt.pool().clone();
    let file_id = Uuid::new_v4();

    backend().put(&pool, file_id, &upload_nonce.app_id, &upload_nonce.name, &upload_nonce.content_type, &body, None).await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok((StatusCode::CREATED, Json(json!({
        "file_id": file_id.to_string(),
        "name": upload_nonce.name,
        "size": body.len(),
    }))))
}

/// POST /api/v1/apps/:app_id/storage/upload — user file upload (requires JWT, scoped by app).
/// Accepts multipart/form-data. Stores in BYTEA. Returns { file_id, name, content_type, size }.
async fn upload_file(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
    mut multipart: Multipart,
) -> Result<(StatusCode, Json<JsonValue>), ApiError> {
    let field = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?
        .ok_or_else(|| ApiError::BadRequest("no file field".into()))?;

    let name = field.file_name().unwrap_or("upload").to_string();
    let content_type = field.content_type().unwrap_or("application/octet-stream").to_string();

    let data = field.bytes().await.map_err(|e| ApiError::BadRequest(e.to_string()))?;
    if data.is_empty() {
        return Err(ApiError::BadRequest("empty file".into()));
    }
    if data.len() > MAX_FILE_BYTES {
        return Err(ApiError::BadRequest(format!("file exceeds limit ({MAX_FILE_BYTES} bytes)")));
    }

    let pool = rt.pool().clone();
    let file_id = Uuid::new_v4();

    backend().put(&pool, file_id, &app_id, &name, &content_type, &data, Some(identity.user_id)).await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok((StatusCode::CREATED, Json(json!({
        "file_id": file_id.to_string(),
        "name": name,
        "content_type": content_type,
        "size": data.len(),
    }))))
}

/// GET /api/v1/apps/:app_id/storage/:file_id — download file (requires JWT, scoped by app)
async fn get_file(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((app_id, file_id)): Path<(String, Uuid)>,
) -> Result<Response, ApiError> {
    let pool = rt.pool().clone();

    let obj = backend().get(&pool, file_id, &app_id).await
        .map_err(|e| match e {
            RuntimeError::NotFound(_) => ApiError::NotFound(format!("file {file_id}")),
            e => ApiError::Internal(e.to_string()),
        })?;

    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, obj.content_type.parse().unwrap_or(header::HeaderValue::from_static("application/octet-stream")));
    let safe_name: String = obj.name.chars().filter(|c| !c.is_control() && *c != '"' && *c != '\\').collect();
    headers.insert(header::CONTENT_DISPOSITION, format!("attachment; filename=\"{}\"", safe_name).parse().unwrap_or(header::HeaderValue::from_static("attachment")));
    headers.insert(header::CONTENT_LENGTH, obj.size.to_string().parse().unwrap());
    headers.insert(header::HeaderName::from_static("x-content-type-options"), header::HeaderValue::from_static("nosniff"));

    Ok((headers, Body::from(obj.content)).into_response())
}

/// DELETE /api/v1/apps/:app_id/storage/:file_id — delete file (requires JWT, scoped by app)
async fn delete_file(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((app_id, file_id)): Path<(String, Uuid)>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = rt.pool().clone();

    backend().delete(&pool, file_id, &app_id).await
        .map_err(|e| match e {
            RuntimeError::NotFound(_) => ApiError::NotFound(format!("file {file_id}")),
            e => ApiError::Internal(e.to_string()),
        })?;

    Ok(Json(json!({ "deleted": file_id.to_string() })))
}
