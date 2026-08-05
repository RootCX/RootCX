mod service;
mod upload;

use axum::Router;
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::RuntimeError;
use crate::routes::SharedRuntime;

pub(crate) const STATUS_PENDING: &str = "pending";
pub(crate) const STATUS_COMPLETED: &str = "completed";
pub(crate) const STATUS_FAILED: &str = "failed";
pub(crate) const STATUS_CANCELLED: &str = "cancelled";

const DEFAULT_IMPORT_MAX_BYTES: i64 = 16 * 1024 * 1024 * 1024;

pub(crate) fn max_import_bytes() -> i64 {
    std::env::var("ROOTCX_IMPORT_MAX_BYTES")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_IMPORT_MAX_BYTES)
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ImportMode {
    Append,
    Upsert,
    Replace,
}

impl ImportMode {
    pub(crate) fn parse(value: &str) -> Result<Self, crate::api_error::ApiError> {
        match value {
            "append" => Ok(Self::Append),
            "upsert" => Ok(Self::Upsert),
            "replace" => Ok(Self::Replace),
            _ => Err(crate::api_error::ApiError::Internal(
                "invalid stored import mode".into(),
            )),
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Append => "append",
            Self::Upsert => "upsert",
            Self::Replace => "replace",
        }
    }

    pub(crate) fn required_actions(self) -> &'static [&'static str] {
        match self {
            Self::Append => &["create"],
            Self::Upsert => &["create", "update"],
            Self::Replace => &["create", "update", "delete"],
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateImportRequest {
    pub(crate) mode: ImportMode,
    pub(crate) columns: Vec<String>,
    #[serde(default)]
    pub(crate) conflict_columns: Vec<String>,
    #[serde(default)]
    pub(crate) allow_empty: bool,
    pub(crate) source_file_id: Option<Uuid>,
    pub(crate) idempotency_key: Option<String>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub(crate) struct CollectionImport {
    pub(crate) id: Uuid,
    pub(crate) app_id: String,
    pub(crate) entity: String,
    pub(crate) mode: String,
    pub(crate) columns: Vec<String>,
    pub(crate) conflict_columns: Vec<String>,
    pub(crate) allow_empty: bool,
    pub(crate) source_file_id: Option<Uuid>,
    pub(crate) source_checksum: Option<String>,
    pub(crate) idempotency_key: Option<String>,
    pub(crate) status: String,
    pub(crate) rows_loaded: i64,
    pub(crate) bytes_received: i64,
    pub(crate) cancel_requested: bool,
    pub(crate) error: Option<String>,
    pub(crate) created_at: String,
    pub(crate) started_at: Option<String>,
    pub(crate) completed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) upload_url: Option<String>,
}

pub(crate) async fn bootstrap(pool: &PgPool) -> Result<(), RuntimeError> {
    for sql in [
        r#"CREATE TABLE IF NOT EXISTS rootcx_system.collection_imports (
            id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            app_id           TEXT NOT NULL REFERENCES rootcx_system.apps(id) ON DELETE CASCADE,
            entity           TEXT NOT NULL,
            mode             TEXT NOT NULL CHECK (mode IN ('append', 'upsert', 'replace')),
            columns          TEXT[] NOT NULL CHECK (cardinality(columns) > 0),
            conflict_columns TEXT[] NOT NULL DEFAULT '{}',
            allow_empty      BOOLEAN NOT NULL DEFAULT false,
            source_file_id   UUID REFERENCES rootcx_system.files(id) ON DELETE RESTRICT,
            source_checksum  TEXT,
            idempotency_key  TEXT,
            status           TEXT NOT NULL DEFAULT 'pending'
                             CHECK (status IN ('pending', 'loading', 'publishing', 'completed', 'failed', 'cancelled')),
            actor_uid        UUID NOT NULL,
            is_delegated     BOOLEAN NOT NULL DEFAULT false,
            effective_perms  TEXT[] NOT NULL DEFAULT '{}',
            token_hash       BYTEA,
            token_expires_at TIMESTAMPTZ,
            staging_table    TEXT,
            rows_loaded      BIGINT NOT NULL DEFAULT 0,
            bytes_received   BIGINT NOT NULL DEFAULT 0,
            cancel_requested BOOLEAN NOT NULL DEFAULT false,
            error            TEXT,
            created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
            started_at       TIMESTAMPTZ,
            completed_at     TIMESTAMPTZ,
            updated_at       TIMESTAMPTZ NOT NULL DEFAULT now()
        )"#,
        "ALTER TABLE rootcx_system.collection_imports ADD COLUMN IF NOT EXISTS allow_empty BOOLEAN NOT NULL DEFAULT false",
        "CREATE UNIQUE INDEX IF NOT EXISTS uq_collection_import_idempotency ON rootcx_system.collection_imports (app_id, entity, actor_uid, idempotency_key) WHERE idempotency_key IS NOT NULL",
        "CREATE UNIQUE INDEX IF NOT EXISTS uq_collection_import_active_target ON rootcx_system.collection_imports (app_id, entity) WHERE status IN ('pending', 'loading', 'publishing')",
        "CREATE INDEX IF NOT EXISTS idx_collection_import_history ON rootcx_system.collection_imports (app_id, entity, created_at DESC)",
        "UPDATE rootcx_system.collection_imports SET status = 'failed', error = 'Core restarted during import', completed_at = now(), updated_at = now(), token_hash = NULL, token_expires_at = NULL, staging_table = NULL WHERE status IN ('loading', 'publishing')",
        "UPDATE rootcx_system.collection_imports SET token_hash = NULL, token_expires_at = NULL WHERE token_expires_at < now()",
    ] {
        sqlx::query(sql)
            .execute(pool)
            .await
            .map_err(RuntimeError::Schema)?;
    }
    Ok(())
}

pub(crate) fn routes() -> Router<SharedRuntime> {
    Router::new()
        .route(
            "/api/v1/apps/{app_id}/collections/{entity}/imports",
            get(service::list_http).post(service::create_http),
        )
        .route(
            "/api/v1/apps/{app_id}/collections/{entity}/imports/{id}",
            get(service::get_http).delete(service::cancel_http),
        )
        .route(
            "/api/v1/apps/{app_id}/collections/{entity}/imports/{id}/retry",
            post(service::retry_http),
        )
        .route(
            "/api/v1/collection-import-uploads/{token}",
            post(upload::upload),
        )
}

pub(crate) use service::create_for_worker;
