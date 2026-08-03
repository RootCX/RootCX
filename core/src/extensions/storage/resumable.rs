use std::time::Duration;

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use sqlx::postgres::types::Oid;
use tokio::sync::{Semaphore, SemaphorePermit};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};
use uuid::Uuid;

use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::routes::SharedRuntime;

use super::large_object::{LargeObjectAppender, checksum_in_transaction};
use super::max_file_bytes;

const MAX_CHUNK_BYTES: usize = 16 * 1024 * 1024;
const MAX_CONCURRENT_TRANSFERS: usize = 4;
const SESSION_TTL_HOURS: i32 = 24;
const CLEANUP_INTERVAL: Duration = Duration::from_secs(60 * 60);
const CHUNK_CONTENT_TYPE: &str = "application/offset+octet-stream";
const _: () = assert!(MAX_CONCURRENT_TRANSFERS < crate::POOL_MAX_CONNECTIONS as usize);
static STORAGE_TRANSFERS: Semaphore = Semaphore::const_new(MAX_CONCURRENT_TRANSFERS);

#[derive(Deserialize)]
struct CreateUpload {
    upload_id: Option<Uuid>,
    name: String,
    content_type: Option<String>,
    size: i64,
}

#[derive(Serialize, sqlx::FromRow)]
struct UploadSession {
    id: Uuid,
    app_id: String,
    name: String,
    content_type: String,
    expected_size: i64,
    uploaded_size: i64,
    completed_file_id: Option<Uuid>,
    created_at: chrono::DateTime<chrono::Utc>,
    expires_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Serialize)]
struct UploadSessionResponse {
    #[serde(flatten)]
    session: UploadSession,
    state: &'static str,
    max_chunk_size: usize,
}

impl From<UploadSession> for UploadSessionResponse {
    fn from(session: UploadSession) -> Self {
        let state = if session.completed_file_id.is_some() {
            "completed"
        } else {
            "uploading"
        };
        Self {
            session,
            state,
            max_chunk_size: MAX_CHUNK_BYTES,
        }
    }
}

#[derive(sqlx::FromRow)]
struct LockedUploadSession {
    name: String,
    content_type: String,
    expected_size: i64,
    uploaded_size: i64,
    content_oid: Option<Oid>,
    completed_file_id: Option<Uuid>,
}

#[derive(Serialize, sqlx::FromRow)]
struct StoredFile {
    file_id: Uuid,
    name: String,
    content_type: String,
    size: i64,
    checksum: String,
}

pub async fn bootstrap(pool: &sqlx::PgPool) -> Result<(), crate::RuntimeError> {
    for statement in [
        r#"
        CREATE TABLE IF NOT EXISTS rootcx_system.storage_upload_sessions (
            id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            app_id            TEXT NOT NULL REFERENCES rootcx_system.apps(id) ON DELETE CASCADE,
            name              TEXT NOT NULL,
            content_type      TEXT NOT NULL DEFAULT 'application/octet-stream',
            expected_size     BIGINT NOT NULL CHECK (expected_size > 0),
            uploaded_size     BIGINT NOT NULL DEFAULT 0 CHECK (uploaded_size >= 0 AND uploaded_size <= expected_size),
            content_oid       OID,
            uploaded_by       UUID NOT NULL REFERENCES rootcx_system.users(id) ON DELETE CASCADE,
            completed_file_id UUID REFERENCES rootcx_system.files(id) ON DELETE CASCADE,
            completed_at      TIMESTAMPTZ,
            created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
            expires_at        TIMESTAMPTZ NOT NULL,
            CONSTRAINT storage_upload_session_state CHECK (
                (content_oid IS NOT NULL AND completed_file_id IS NULL AND completed_at IS NULL)
                OR
                (content_oid IS NULL AND completed_file_id IS NOT NULL AND completed_at IS NOT NULL)
            )
        )
        "#,
        "CREATE INDEX IF NOT EXISTS idx_storage_upload_sessions_expiry ON rootcx_system.storage_upload_sessions (expires_at)",
        r#"
        CREATE OR REPLACE FUNCTION rootcx_system.unlink_storage_upload_session_large_object()
        RETURNS trigger LANGUAGE plpgsql AS $$
        BEGIN
            IF OLD.content_oid IS NOT NULL THEN
                PERFORM lo_unlink(OLD.content_oid);
            END IF;
            RETURN OLD;
        END $$
        "#,
        "DROP TRIGGER IF EXISTS trg_unlink_storage_upload_session_large_object ON rootcx_system.storage_upload_sessions",
        "CREATE TRIGGER trg_unlink_storage_upload_session_large_object BEFORE DELETE ON rootcx_system.storage_upload_sessions FOR EACH ROW EXECUTE FUNCTION rootcx_system.unlink_storage_upload_session_large_object()",
    ] {
        sqlx::query(statement)
            .execute(pool)
            .await
            .map_err(crate::RuntimeError::Schema)?;
    }
    Ok(())
}

pub fn spawn_cleanup(pool: sqlx::PgPool, cancel: CancellationToken) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(CLEANUP_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = interval.tick() => match cleanup_expired(&pool).await {
                    Ok(0) => {}
                    Ok(count) => info!(count, "expired storage upload sessions removed"),
                    Err(error) => warn!(%error, "failed to clean expired storage upload sessions"),
                },
            }
        }
    });
}

async fn cleanup_expired(pool: &sqlx::PgPool) -> Result<u64, sqlx::Error> {
    Ok(
        sqlx::query("DELETE FROM rootcx_system.storage_upload_sessions WHERE expires_at <= now()")
            .execute(pool)
            .await?
            .rows_affected(),
    )
}

async fn acquire_transfer_slot() -> Result<SemaphorePermit<'static>, ApiError> {
    STORAGE_TRANSFERS
        .acquire()
        .await
        .map_err(|_| ApiError::Internal("upload service unavailable".into()))
}

pub fn routes() -> Router<SharedRuntime> {
    Router::new()
        .route("/api/v1/apps/{app_id}/storage/uploads", post(create_upload))
        .route(
            "/api/v1/apps/{app_id}/storage/uploads/{upload_id}",
            get(get_upload)
                .patch(append_chunk)
                .delete(cancel_upload)
                .layer(DefaultBodyLimit::max(MAX_CHUNK_BYTES)),
        )
        .route(
            "/api/v1/apps/{app_id}/storage/uploads/{upload_id}/complete",
            post(complete_upload),
        )
}

async fn create_upload(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
    Json(input): Json<CreateUpload>,
) -> Result<(StatusCode, Json<UploadSessionResponse>), ApiError> {
    require_write(rt.pool(), identity.user_id, &app_id).await?;
    let name = validate_name(&input.name)?;
    let content_type = validate_content_type(input.content_type.as_deref())?;
    validate_file_size(input.size, max_file_bytes() as i64)?;

    let upload_id = input.upload_id.unwrap_or_else(Uuid::new_v4);
    let mut tx = rt.pool().begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(upload_id.to_string())
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM rootcx_system.storage_upload_sessions WHERE expires_at <= now()")
        .execute(&mut *tx)
        .await?;
    let existing = sqlx::query_as::<_, UploadSession>(
        r#"
        SELECT id, app_id, name, content_type, expected_size, uploaded_size,
               completed_file_id, created_at, expires_at
        FROM rootcx_system.storage_upload_sessions
        WHERE id = $1 AND app_id = $2 AND uploaded_by = $3
        "#,
    )
    .bind(upload_id)
    .bind(&app_id)
    .bind(identity.user_id)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(session) = existing {
        if session.name != name
            || session.content_type != content_type
            || session.expected_size != input.size
        {
            return Err(ApiError::Conflict(
                "upload id already exists with different metadata".into(),
            ));
        }
        tx.commit().await?;
        return Ok((StatusCode::OK, Json(session.into())));
    }
    let id_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM rootcx_system.storage_upload_sessions WHERE id = $1)",
    )
    .bind(upload_id)
    .fetch_one(&mut *tx)
    .await?;
    if id_exists {
        return Err(ApiError::Conflict("upload id is unavailable".into()));
    }
    let oid = sqlx::query_scalar::<_, Oid>("SELECT lo_create(0)")
        .fetch_one(&mut *tx)
        .await?;
    let session = sqlx::query_as::<_, UploadSession>(
        r#"
        INSERT INTO rootcx_system.storage_upload_sessions
            (id, app_id, name, content_type, expected_size, content_oid, uploaded_by, expires_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, now() + make_interval(hours => $8))
        RETURNING id, app_id, name, content_type, expected_size, uploaded_size,
                  completed_file_id, created_at, expires_at
        "#,
    )
    .bind(upload_id)
    .bind(&app_id)
    .bind(name)
    .bind(content_type)
    .bind(input.size)
    .bind(oid)
    .bind(identity.user_id)
    .bind(SESSION_TTL_HOURS)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(session.into())))
}

async fn get_upload(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((app_id, upload_id)): Path<(String, Uuid)>,
) -> Result<Json<UploadSessionResponse>, ApiError> {
    require_write(rt.pool(), identity.user_id, &app_id).await?;
    let session = sqlx::query_as::<_, UploadSession>(
        r#"
        SELECT id, app_id, name, content_type, expected_size, uploaded_size,
               completed_file_id, created_at, expires_at
        FROM rootcx_system.storage_upload_sessions
        WHERE id = $1 AND app_id = $2 AND uploaded_by = $3 AND expires_at > now()
        "#,
    )
    .bind(upload_id)
    .bind(app_id)
    .bind(identity.user_id)
    .fetch_optional(rt.pool())
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("upload {upload_id}")))?;
    Ok(Json(session.into()))
}

async fn append_chunk(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((app_id, upload_id)): Path<(String, Uuid)>,
    headers: HeaderMap,
    body: Body,
) -> Result<Json<JsonValue>, ApiError> {
    require_write(rt.pool(), identity.user_id, &app_id).await?;
    if headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        != Some(CHUNK_CONTENT_TYPE)
    {
        return Err(ApiError::BadRequest(format!(
            "Content-Type must be {CHUNK_CONTENT_TYPE}"
        )));
    }
    let requested_offset = headers
        .get("upload-offset")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|offset| *offset >= 0)
        .ok_or_else(|| ApiError::BadRequest("missing or invalid Upload-Offset header".into()))?;
    let _permit = acquire_transfer_slot().await?;
    let tx = rt.pool().begin().await?;
    let (session, mut appender) = lock_upload(tx, upload_id, &app_id, identity.user_id).await?;
    if requested_offset != session.uploaded_size {
        return Err(ApiError::Conflict(format!(
            "upload offset mismatch: expected {}, got {requested_offset}",
            session.uploaded_size
        )));
    }

    let mut stream = body.into_data_stream();
    let mut chunk_size = 0_i64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| ApiError::BadRequest(error.to_string()))?;
        chunk_size += chunk.len() as i64;
        if chunk_size > MAX_CHUNK_BYTES as i64 {
            return Err(ApiError::BadRequest(format!(
                "chunk exceeds {MAX_CHUNK_BYTES} bytes"
            )));
        }
        if requested_offset + chunk_size > session.expected_size {
            return Err(ApiError::BadRequest(
                "chunk exceeds declared file size".into(),
            ));
        }
        appender.write(&chunk).await?;
    }
    if chunk_size == 0 {
        return Err(ApiError::BadRequest("empty upload chunk".into()));
    }

    let uploaded_size = appender.position();
    sqlx::query(
        r#"
        UPDATE rootcx_system.storage_upload_sessions
        SET uploaded_size = $1, expires_at = now() + make_interval(hours => $2)
        WHERE id = $3
        "#,
    )
    .bind(uploaded_size)
    .bind(SESSION_TTL_HOURS)
    .bind(upload_id)
    .execute(appender.connection())
    .await?;
    appender.commit().await?;
    Ok(Json(
        json!({ "upload_id": upload_id, "uploaded_size": uploaded_size }),
    ))
}

async fn lock_upload(
    mut tx: sqlx::Transaction<'static, sqlx::Postgres>,
    upload_id: Uuid,
    app_id: &str,
    user_id: Uuid,
) -> Result<(LockedUploadSession, LargeObjectAppender), ApiError> {
    let session = select_locked_upload(&mut tx, upload_id, app_id, user_id).await?;
    if session.completed_file_id.is_some() {
        return Err(ApiError::Conflict("upload already completed".into()));
    }
    let oid = session
        .content_oid
        .ok_or_else(|| ApiError::Internal("active upload has no content".into()))?;
    let offset = session.uploaded_size;
    let appender = LargeObjectAppender::open(tx, oid, offset).await?;
    Ok((session, appender))
}

async fn select_locked_upload(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    upload_id: Uuid,
    app_id: &str,
    user_id: Uuid,
) -> Result<LockedUploadSession, ApiError> {
    sqlx::query_as::<_, LockedUploadSession>(
        r#"
        SELECT name, content_type, expected_size, uploaded_size, content_oid, completed_file_id
        FROM rootcx_system.storage_upload_sessions
        WHERE id = $1 AND app_id = $2 AND uploaded_by = $3 AND expires_at > now()
        FOR UPDATE
        "#,
    )
    .bind(upload_id)
    .bind(app_id)
    .bind(user_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("upload {upload_id}")))
}

async fn complete_upload(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((app_id, upload_id)): Path<(String, Uuid)>,
) -> Result<(StatusCode, Json<StoredFile>), ApiError> {
    require_write(rt.pool(), identity.user_id, &app_id).await?;
    let _permit = acquire_transfer_slot().await?;
    let mut tx = rt.pool().begin().await?;
    let session = select_locked_upload(&mut tx, upload_id, &app_id, identity.user_id).await?;
    if let Some(file_id) = session.completed_file_id {
        let file = load_stored_file(&mut tx, file_id, &app_id).await?;
        tx.commit().await?;
        return Ok((StatusCode::OK, Json(file)));
    }
    if session.uploaded_size != session.expected_size {
        return Err(ApiError::BadRequest(format!(
            "upload incomplete: {}/{} bytes",
            session.uploaded_size, session.expected_size
        )));
    }
    let oid = session
        .content_oid
        .ok_or_else(|| ApiError::Internal("active upload has no content".into()))?;
    let checksum = checksum_in_transaction(&mut tx, oid, session.uploaded_size).await?;
    let file = StoredFile {
        file_id: Uuid::new_v4(),
        name: session.name,
        content_type: session.content_type,
        size: session.uploaded_size,
        checksum,
    };
    sqlx::query(
        r#"
        INSERT INTO rootcx_system.files
            (id, app_id, name, content_type, size, content, content_oid, checksum, uploaded_by)
        VALUES ($1, $2, $3, $4, $5, NULL, $6, $7, $8)
        "#,
    )
    .bind(file.file_id)
    .bind(&app_id)
    .bind(&file.name)
    .bind(&file.content_type)
    .bind(file.size)
    .bind(oid)
    .bind(&file.checksum)
    .bind(identity.user_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"
        UPDATE rootcx_system.storage_upload_sessions
        SET content_oid = NULL, completed_file_id = $1, completed_at = now(),
            expires_at = now() + make_interval(hours => $2)
        WHERE id = $3
        "#,
    )
    .bind(file.file_id)
    .bind(SESSION_TTL_HOURS)
    .bind(upload_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(file)))
}

async fn load_stored_file(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    file_id: Uuid,
    app_id: &str,
) -> Result<StoredFile, ApiError> {
    sqlx::query_as::<_, StoredFile>(
        r#"
        SELECT id AS file_id, name, content_type, size, checksum
        FROM rootcx_system.files
        WHERE id = $1 AND app_id = $2
        "#,
    )
    .bind(file_id)
    .bind(app_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("file {file_id}")))
}

async fn cancel_upload(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((app_id, upload_id)): Path<(String, Uuid)>,
) -> Result<Json<JsonValue>, ApiError> {
    require_write(rt.pool(), identity.user_id, &app_id).await?;
    let mut tx = rt.pool().begin().await?;
    let session = select_locked_upload(&mut tx, upload_id, &app_id, identity.user_id).await?;
    if session.completed_file_id.is_some() {
        return Err(ApiError::Conflict(
            "completed upload cannot be cancelled".into(),
        ));
    }
    sqlx::query("DELETE FROM rootcx_system.storage_upload_sessions WHERE id = $1")
        .bind(upload_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(json!({ "deleted": upload_id })))
}

fn validate_name(value: &str) -> Result<String, ApiError> {
    let name = value.trim();
    if name.is_empty() || name.len() > 255 || name.chars().any(char::is_control) {
        return Err(ApiError::BadRequest("invalid file name".into()));
    }
    Ok(name.to_string())
}

fn validate_file_size(size: i64, limit: i64) -> Result<(), ApiError> {
    if size <= 0 || size > limit {
        return Err(ApiError::BadRequest(format!(
            "file size must be between 1 and {limit} bytes"
        )));
    }
    Ok(())
}

async fn require_write(pool: &sqlx::PgPool, user_id: Uuid, app_id: &str) -> Result<(), ApiError> {
    crate::governance::authority::require_perm(
        pool,
        user_id,
        &format!("app:{app_id}:storage.write"),
    )
    .await
}

fn validate_content_type(value: Option<&str>) -> Result<String, ApiError> {
    let Some(content_type) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok("application/octet-stream".into());
    };
    if content_type.len() > 255 || content_type.chars().any(char::is_control) {
        return Err(ApiError::BadRequest("invalid content type".into()));
    }
    Ok(content_type.to_string())
}

#[cfg(test)]
mod tests {
    use super::{validate_content_type, validate_file_size, validate_name};

    #[test]
    fn file_names_reject_empty_controlled_or_oversized_values() {
        for value in ["", "  ", "bad\nname", &"x".repeat(256)] {
            assert!(
                validate_name(value).is_err(),
                "accepted invalid name: {value:?}"
            );
        }
        assert_eq!(validate_name(" catalog.xlsx ").unwrap(), "catalog.xlsx");
        assert!(validate_name(&"x".repeat(255)).is_ok());
    }

    #[test]
    fn content_types_default_only_when_absent_or_empty() {
        for value in [None, Some(""), Some("  ")] {
            assert_eq!(
                validate_content_type(value).unwrap(),
                "application/octet-stream"
            );
        }
        for value in ["bad\rvalue", &"x".repeat(256)] {
            assert!(
                validate_content_type(Some(value)).is_err(),
                "accepted invalid content type: {value:?}"
            );
        }
        assert!(validate_content_type(Some(&"x".repeat(255))).is_ok());
    }

    #[test]
    fn file_sizes_enforce_positive_inclusive_limit() {
        let limit = 10;
        for size in [i64::MIN, -1, 0, limit + 1, i64::MAX] {
            assert!(
                validate_file_size(size, limit).is_err(),
                "accepted invalid size: {size}"
            );
        }
        for size in [1, limit] {
            assert!(
                validate_file_size(size, limit).is_ok(),
                "rejected valid size: {size}"
            );
        }
    }
}
