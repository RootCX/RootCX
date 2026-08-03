pub mod backend;
pub(crate) mod large_object;
pub mod nonce;
mod resumable;
#[cfg(test)]
#[path = "backend_test.rs"]
mod backend_test;

use async_trait::async_trait;
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Multipart, Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::StreamExt;
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

const DEFAULT_MAX_FILE_BYTES: usize = 1024 * 1024 * 1024;

pub(crate) fn max_file_bytes() -> usize {
    std::env::var("ROOTCX_STORAGE_MAX_FILE_BYTES")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_MAX_FILE_BYTES)
}

pub(crate) fn spawn_upload_cleanup(
    pool: PgPool,
    cancel: tokio_util::sync::CancellationToken,
) {
    resumable::spawn_cleanup(pool, cancel);
}

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
                content      BYTEA,
                content_oid  OID,
                checksum     TEXT,
                uploaded_by  UUID,
                created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
            )
        "#).await?;

        exec(pool, "ALTER TABLE rootcx_system.files ALTER COLUMN content DROP NOT NULL").await?;
        exec(pool, "ALTER TABLE rootcx_system.files ADD COLUMN IF NOT EXISTS content_oid OID").await?;
        exec(pool, "ALTER TABLE rootcx_system.files ADD COLUMN IF NOT EXISTS checksum TEXT").await?;
        exec(pool, "CREATE INDEX IF NOT EXISTS idx_files_app ON rootcx_system.files (app_id)").await?;
        exec(pool, "CREATE INDEX IF NOT EXISTS idx_files_created ON rootcx_system.files (created_at DESC)").await?;
        exec(pool, r#"
            CREATE OR REPLACE FUNCTION rootcx_system.unlink_file_large_object()
            RETURNS trigger LANGUAGE plpgsql AS $$
            BEGIN
                IF OLD.content_oid IS NOT NULL THEN
                    PERFORM lo_unlink(OLD.content_oid);
                END IF;
                RETURN OLD;
            END $$
        "#).await?;
        exec(pool, "DROP TRIGGER IF EXISTS trg_unlink_file_large_object ON rootcx_system.files").await?;
        exec(pool, "CREATE TRIGGER trg_unlink_file_large_object BEFORE DELETE ON rootcx_system.files FOR EACH ROW EXECUTE FUNCTION rootcx_system.unlink_file_large_object()").await?;
        exec(pool, r#"
            INSERT INTO rootcx_system.rbac_permissions (key, description, source_app)
            SELECT 'app:' || app.id || ':storage.' || permission.action,
                   permission.description,
                   app.id
            FROM rootcx_system.apps app
            CROSS JOIN (VALUES
                ('read', 'read app files'),
                ('write', 'write app files')
            ) AS permission(action, description)
            ON CONFLICT (key) DO NOTHING
        "#).await?;
        resumable::bootstrap(pool).await?;

        info!("storage extension ready");
        Ok(())
    }

    fn routes(&self) -> Option<Router<SharedRuntime>> {
        Some(
            Router::new()
                // Nonce-authenticated upload for workers (no Identity required)
                .route("/api/v1/storage/upload/{nonce}", post(upload_via_nonce).layer(DefaultBodyLimit::max(max_file_bytes())))
                // Nonce-authenticated download for workers (no Identity required)
                .route("/api/v1/storage/download/{nonce}", get(download_via_nonce))
                // JWT-authenticated upload for users/frontend — scoped by app_id
                .route("/api/v1/apps/{app_id}/storage/upload", post(upload_file).layer(DefaultBodyLimit::max(max_file_bytes())))
                // JWT-authenticated download/delete — scoped by app_id
                .route("/api/v1/apps/{app_id}/storage/{file_id}", get(get_file).delete(delete_file))
                .merge(resumable::routes())
        )
    }
}

fn backend() -> PostgresBackend {
    PostgresBackend
}

async fn open_file(pool: &PgPool, file_id: Uuid, app_id: &str) -> Result<backend::StorageDownload, ApiError> {
    backend().open(pool, file_id, app_id).await
        .map_err(|e| match e {
            crate::RuntimeError::NotFound(_) => ApiError::NotFound(format!("file {file_id}")),
            e => ApiError::Internal(e.to_string()),
        })
}

/// Build a one-time nonce download URL for the given nonce.
pub fn download_url(runtime_url: &str, nonce: &str) -> String {
    format!("{runtime_url}/api/v1/storage/download/{nonce}")
}

/// POST /api/v1/storage/upload/{nonce} — worker upload via single-use nonce.
/// No JWT required. The nonce proves the upload was authorized by Core via IPC.
async fn upload_via_nonce(
    State(rt): State<SharedRuntime>,
    Path(nonce_id): Path<String>,
    body: Body,
) -> Result<(StatusCode, Json<JsonValue>), ApiError> {
    let upload_nonce = rt.upload_nonces().lock().unwrap_or_else(|e| e.into_inner()).consume(&nonce_id)
        .ok_or_else(|| ApiError::NotFound("invalid or expired upload nonce".into()))?;

    let pool = rt.pool().clone();
    let file_id = Uuid::new_v4();
    let storage_limit = max_file_bytes() as i64;
    let mut writer = backend().begin(&pool).await?;
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| ApiError::BadRequest(e.to_string()))?;
        let next_size = writer.size() + chunk.len() as i64;
        if next_size > storage_limit {
            return Err(ApiError::BadRequest(format!("file exceeds storage limit ({storage_limit} bytes)")));
        }
        if upload_nonce.max_size > 0 && next_size > upload_nonce.max_size as i64 {
            return Err(ApiError::BadRequest(format!("file exceeds declared size ({} bytes)", upload_nonce.max_size)));
        }
        writer.write(&chunk).await?;
    }
    if writer.size() == 0 {
        return Err(ApiError::BadRequest("empty file".into()));
    }
    let upload = backend().finish(writer, file_id, &upload_nonce.app_id, &upload_nonce.name, &upload_nonce.content_type, None).await?;

    Ok((StatusCode::CREATED, Json(json!({
        "file_id": file_id.to_string(),
        "name": upload_nonce.name,
        "size": upload.size,
        "checksum": upload.checksum,
    }))))
}

/// POST /api/v1/apps/:app_id/storage/upload — user file upload (requires JWT, scoped by app).
/// Accepts multipart/form-data. Streams to a PostgreSQL Large Object.
async fn upload_file(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
    mut multipart: Multipart,
) -> Result<(StatusCode, Json<JsonValue>), ApiError> {
    let mut field = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::BadRequest(e.to_string()))?
        .ok_or_else(|| ApiError::BadRequest("no file field".into()))?;

    let name = field.file_name().unwrap_or("upload").to_string();
    let content_type = field.content_type().unwrap_or("application/octet-stream").to_string();

    let pool = rt.pool().clone();
    let file_id = Uuid::new_v4();
    let storage_limit = max_file_bytes() as i64;
    let mut writer = backend().begin(&pool).await?;
    while let Some(chunk) = field.chunk().await.map_err(|e| ApiError::BadRequest(e.to_string()))? {
        if writer.size() + chunk.len() as i64 > storage_limit {
            return Err(ApiError::BadRequest(format!("file exceeds storage limit ({storage_limit} bytes)")));
        }
        writer.write(&chunk).await?;
    }
    if writer.size() == 0 {
        return Err(ApiError::BadRequest("empty file".into()));
    }
    let upload = backend().finish(writer, file_id, &app_id, &name, &content_type, Some(identity.user_id)).await?;

    Ok((StatusCode::CREATED, Json(json!({
        "file_id": file_id.to_string(),
        "name": name,
        "content_type": content_type,
        "size": upload.size,
        "checksum": upload.checksum,
    }))))
}

/// GET /api/v1/storage/download/{nonce} — worker download via single-use nonce.
/// No JWT required. Used by agent workers to fetch file attachments.
async fn download_via_nonce(
    State(rt): State<SharedRuntime>,
    Path(nonce_id): Path<String>,
) -> Result<Response, ApiError> {
    let (file_id, app_id) = {
        let mut store = rt.upload_nonces().lock().unwrap_or_else(|e| e.into_inner());
        let nonce = store.consume_download(&nonce_id)
            .ok_or_else(|| ApiError::NotFound("invalid or expired download nonce".into()))?;
        (nonce.file_id, nonce.app_id)
    };
    let obj = open_file(rt.pool(), file_id, &app_id).await?;
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, obj.content_type.parse().unwrap_or(header::HeaderValue::from_static("application/octet-stream")));
    headers.insert(header::CONTENT_LENGTH, obj.size.to_string().parse().unwrap());
    Ok((headers, obj.body).into_response())
}

/// GET /api/v1/apps/:app_id/storage/:file_id — download file (requires JWT, scoped by app)
async fn get_file(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((app_id, file_id)): Path<(String, Uuid)>,
) -> Result<Response, ApiError> {
    let obj = open_file(rt.pool(), file_id, &app_id).await?;
    let safe_name: String = obj.name.chars().filter(|c| !c.is_control() && *c != '"' && *c != '\\').collect();
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, obj.content_type.parse().unwrap_or(header::HeaderValue::from_static("application/octet-stream")));
    headers.insert(header::CONTENT_DISPOSITION, format!("attachment; filename=\"{}\"", safe_name).parse().unwrap_or(header::HeaderValue::from_static("attachment")));
    headers.insert(header::CONTENT_LENGTH, obj.size.to_string().parse().unwrap());
    headers.insert(header::HeaderName::from_static("x-content-type-options"), header::HeaderValue::from_static("nosniff"));
    Ok((headers, obj.body).into_response())
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
