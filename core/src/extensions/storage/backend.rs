use async_trait::async_trait;
use tokio_util::bytes::Bytes;
use sqlx::PgPool;
use uuid::Uuid;

use crate::RuntimeError;

#[async_trait]
pub trait StorageBackend: Send + Sync {
    async fn put(&self, pool: &PgPool, id: Uuid, app_id: &str, name: &str, content_type: &str, data: &[u8], uploaded_by: Option<Uuid>) -> Result<(), RuntimeError>;
    async fn get(&self, pool: &PgPool, id: Uuid, app_id: &str) -> Result<StorageObject, RuntimeError>;
    async fn delete(&self, pool: &PgPool, id: Uuid, app_id: &str) -> Result<(), RuntimeError>;
}

pub struct StorageObject {
    pub content: Bytes,
    pub content_type: String,
    pub name: String,
    pub size: i64,
}

/// Postgres BYTEA backend — stores file content directly in rootcx_system.files.
/// TOAST compression handles large values transparently (XML compresses ~5-10x).
pub struct PostgresBackend;

#[async_trait]
impl StorageBackend for PostgresBackend {
    async fn put(&self, pool: &PgPool, id: Uuid, app_id: &str, name: &str, content_type: &str, data: &[u8], uploaded_by: Option<Uuid>) -> Result<(), RuntimeError> {
        let size = data.len() as i64;
        sqlx::query(
            "INSERT INTO rootcx_system.files (id, app_id, name, content_type, size, content, uploaded_by) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)"
        )
            .bind(id)
            .bind(app_id)
            .bind(name)
            .bind(content_type)
            .bind(size)
            .bind(data)
            .bind(uploaded_by)
            .execute(pool)
            .await
            .map_err(RuntimeError::Schema)?;
        Ok(())
    }

    async fn get(&self, pool: &PgPool, id: Uuid, app_id: &str) -> Result<StorageObject, RuntimeError> {
        let row: (Vec<u8>, String, String, i64) = sqlx::query_as(
            "SELECT content, content_type, name, size FROM rootcx_system.files WHERE id = $1 AND app_id = $2"
        )
            .bind(id)
            .bind(app_id)
            .fetch_optional(pool)
            .await
            .map_err(RuntimeError::Schema)?
            .ok_or_else(|| RuntimeError::NotFound(format!("file {id}")))?;

        Ok(StorageObject {
            content: Bytes::from(row.0),
            content_type: row.1,
            name: row.2,
            size: row.3,
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
