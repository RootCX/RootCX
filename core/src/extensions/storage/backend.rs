use async_trait::async_trait;
use axum::body::{Body, Bytes};
use sqlx::postgres::types::Oid;
use sqlx::PgPool;
use uuid::Uuid;

use crate::RuntimeError;

use super::large_object::{LargeObjectReader, LargeObjectWriter};

const MAX_BUFFERED_FILE_BYTES: i64 = 64 * 1024 * 1024;
type StoredFileRow = (Option<Vec<u8>>, Option<Oid>, String, String, i64);

fn protocol_error(message: impl Into<String>) -> RuntimeError {
    RuntimeError::Schema(sqlx::Error::Protocol(message.into()))
}

#[async_trait]
pub trait StorageBackend: Send + Sync {
    async fn put(
        &self,
        pool: &PgPool,
        id: Uuid,
        app_id: &str,
        name: &str,
        content_type: &str,
        data: &[u8],
        uploaded_by: Option<Uuid>,
    ) -> Result<(), RuntimeError>;
    async fn get(
        &self,
        pool: &PgPool,
        id: Uuid,
        app_id: &str,
    ) -> Result<StorageObject, RuntimeError>;
    async fn open(
        &self,
        pool: &PgPool,
        id: Uuid,
        app_id: &str,
    ) -> Result<StorageDownload, RuntimeError>;
    async fn delete(&self, pool: &PgPool, id: Uuid, app_id: &str) -> Result<(), RuntimeError>;
}

pub struct StorageObject {
    pub content: Bytes,
    pub content_type: String,
    pub name: String,
    pub size: i64,
}

pub struct StorageDownload {
    pub body: Body,
    pub content_type: String,
    pub name: String,
    pub size: i64,
}

pub struct StoredUpload {
    pub size: i64,
    pub checksum: String,
}

pub struct PostgresBackend;

impl PostgresBackend {
    pub async fn begin(&self, pool: &PgPool) -> Result<LargeObjectWriter, RuntimeError> {
        LargeObjectWriter::create(pool).await
    }

    pub async fn finish(
        &self,
        mut writer: LargeObjectWriter,
        id: Uuid,
        app_id: &str,
        name: &str,
        content_type: &str,
        uploaded_by: Option<Uuid>,
    ) -> Result<StoredUpload, RuntimeError> {
        let upload = StoredUpload {
            size: writer.size(),
            checksum: writer.checksum(),
        };
        sqlx::query(
            "INSERT INTO rootcx_system.files \
             (id, app_id, name, content_type, size, content, content_oid, checksum, uploaded_by) \
             VALUES ($1, $2, $3, $4, $5, NULL, $6, $7, $8)",
        )
        .bind(id)
        .bind(app_id)
        .bind(name)
        .bind(content_type)
        .bind(upload.size)
        .bind(writer.oid())
        .bind(&upload.checksum)
        .bind(uploaded_by)
        .execute(writer.connection())
        .await
        .map_err(RuntimeError::Schema)?;
        writer.commit().await?;
        Ok(upload)
    }

    async fn row(
        &self,
        pool: &PgPool,
        id: Uuid,
        app_id: &str,
    ) -> Result<StoredFileRow, RuntimeError> {
        sqlx::query_as(
            "SELECT content, content_oid, content_type, name, size \
             FROM rootcx_system.files WHERE id = $1 AND app_id = $2",
        )
        .bind(id)
        .bind(app_id)
        .fetch_optional(pool)
        .await
        .map_err(RuntimeError::Schema)?
        .ok_or_else(|| RuntimeError::NotFound(format!("file {id}")))
    }
}

#[async_trait]
impl StorageBackend for PostgresBackend {
    async fn put(
        &self,
        pool: &PgPool,
        id: Uuid,
        app_id: &str,
        name: &str,
        content_type: &str,
        data: &[u8],
        uploaded_by: Option<Uuid>,
    ) -> Result<(), RuntimeError> {
        let mut writer = self.begin(pool).await?;
        writer.write(data).await?;
        self.finish(writer, id, app_id, name, content_type, uploaded_by)
            .await?;
        Ok(())
    }

    async fn get(
        &self,
        pool: &PgPool,
        id: Uuid,
        app_id: &str,
    ) -> Result<StorageObject, RuntimeError> {
        let (inline, oid, content_type, name, size) = self.row(pool, id, app_id).await?;
        let content = match (inline, oid) {
            (Some(content), _) => Bytes::from(content),
            (None, Some(oid)) => {
                if size > MAX_BUFFERED_FILE_BYTES {
                    return Err(protocol_error(format!(
                        "file {id} is too large for buffered access; use streaming open"
                    )));
                }
                LargeObjectReader::open(pool, oid, size)
                    .await?
                    .read_all()
                    .await?
            }
            (None, None) => return Err(protocol_error(format!("file {id} has no content"))),
        };
        Ok(StorageObject {
            content,
            content_type,
            name,
            size,
        })
    }

    async fn open(
        &self,
        pool: &PgPool,
        id: Uuid,
        app_id: &str,
    ) -> Result<StorageDownload, RuntimeError> {
        let (inline, oid, content_type, name, size) = self.row(pool, id, app_id).await?;
        let body = match (inline, oid) {
            (Some(content), _) => Body::from(content),
            (None, Some(oid)) => LargeObjectReader::open(pool, oid, size).await?.into_body(),
            (None, None) => return Err(protocol_error(format!("file {id} has no content"))),
        };
        Ok(StorageDownload {
            body,
            content_type,
            name,
            size,
        })
    }

    async fn delete(&self, pool: &PgPool, id: Uuid, app_id: &str) -> Result<(), RuntimeError> {
        let result = sqlx::query("DELETE FROM rootcx_system.files WHERE id = $1 AND app_id = $2")
            .bind(id)
            .bind(app_id)
            .execute(pool)
            .await
            .map_err(RuntimeError::Schema)?;
        if result.rows_affected() == 0 {
            return Err(RuntimeError::NotFound(format!("file {id}")));
        }
        Ok(())
    }
}
