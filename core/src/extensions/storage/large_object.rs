use axum::body::{Body, Bytes};
use futures::stream;
use sha2::{Digest, Sha256};
use sqlx::postgres::types::Oid;
use sqlx::{PgConnection, PgPool, Postgres, Transaction};

use crate::RuntimeError;

const INV_WRITE: i32 = 0x0002_0000;
const INV_READ: i32 = 0x0004_0000;
const SEEK_SET: i32 = 0;
const WRITE_CHUNK_BYTES: usize = 1024 * 1024;
const READ_CHUNK_BYTES: i32 = 1024 * 1024;

fn schema_error(error: sqlx::Error) -> RuntimeError {
    RuntimeError::Schema(error)
}

fn protocol_error(message: impl Into<String>) -> RuntimeError {
    schema_error(sqlx::Error::Protocol(message.into()))
}

pub struct LargeObjectWriter {
    tx: Transaction<'static, Postgres>,
    fd: i32,
    oid: Oid,
    size: i64,
    hasher: Sha256,
    pending: Vec<u8>,
}

impl LargeObjectWriter {
    pub async fn create(pool: &PgPool) -> Result<Self, RuntimeError> {
        let mut tx = pool.begin().await.map_err(schema_error)?;
        let oid = sqlx::query_scalar::<_, Oid>("SELECT lo_create(0)")
            .fetch_one(&mut *tx)
            .await
            .map_err(schema_error)?;
        let fd = sqlx::query_scalar::<_, i32>("SELECT lo_open($1, $2)")
            .bind(oid)
            .bind(INV_WRITE)
            .fetch_one(&mut *tx)
            .await
            .map_err(schema_error)?;
        Ok(Self {
            tx,
            fd,
            oid,
            size: 0,
            hasher: Sha256::new(),
            pending: Vec::with_capacity(WRITE_CHUNK_BYTES),
        })
    }

    pub async fn write(&mut self, mut chunk: &[u8]) -> Result<(), RuntimeError> {
        if chunk.is_empty() {
            return Ok(());
        }
        self.size += chunk.len() as i64;
        self.hasher.update(chunk);
        while !chunk.is_empty() {
            let length = chunk.len().min(WRITE_CHUNK_BYTES - self.pending.len());
            self.pending.extend_from_slice(&chunk[..length]);
            chunk = &chunk[length..];
            if self.pending.len() == WRITE_CHUNK_BYTES {
                self.flush().await?;
            }
        }
        Ok(())
    }

    async fn flush(&mut self) -> Result<(), RuntimeError> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let expected = self.pending.len();
        let written = sqlx::query_scalar::<_, i32>("SELECT lowrite($1, $2)")
            .bind(self.fd)
            .bind(&self.pending)
            .fetch_one(&mut *self.tx)
            .await
            .map_err(schema_error)?;
        if written as usize != expected {
            return Err(protocol_error(format!(
                "large object short write: {written}/{}",
                expected
            )));
        }
        self.pending.clear();
        Ok(())
    }

    pub fn oid(&self) -> Oid {
        self.oid
    }
    pub fn size(&self) -> i64 {
        self.size
    }
    pub fn checksum(&self) -> String {
        hex::encode(self.hasher.clone().finalize())
    }
    pub fn connection(&mut self) -> &mut PgConnection {
        &mut self.tx
    }

    pub async fn commit(mut self) -> Result<(), RuntimeError> {
        self.flush().await?;
        sqlx::query_scalar::<_, i32>("SELECT lo_close($1)")
            .bind(self.fd)
            .fetch_one(&mut *self.tx)
            .await
            .map_err(schema_error)?;
        self.tx.commit().await.map_err(schema_error)
    }
}

pub struct LargeObjectAppender {
    tx: Transaction<'static, Postgres>,
    fd: i32,
    position: i64,
    pending: Vec<u8>,
}

impl LargeObjectAppender {
    pub async fn open(
        mut tx: Transaction<'static, Postgres>,
        oid: Oid,
        offset: i64,
    ) -> Result<Self, RuntimeError> {
        let fd = sqlx::query_scalar::<_, i32>("SELECT lo_open($1, $2)")
            .bind(oid)
            .bind(INV_WRITE)
            .fetch_one(&mut *tx)
            .await
            .map_err(schema_error)?;
        let position = sqlx::query_scalar::<_, i64>("SELECT lo_lseek64($1, $2, $3)")
            .bind(fd)
            .bind(offset)
            .bind(SEEK_SET)
            .fetch_one(&mut *tx)
            .await
            .map_err(schema_error)?;
        if position != offset {
            return Err(protocol_error(format!(
                "large object seek returned {position}, expected {offset}"
            )));
        }
        Ok(Self {
            tx,
            fd,
            position,
            pending: Vec::with_capacity(WRITE_CHUNK_BYTES),
        })
    }

    pub async fn write(&mut self, mut chunk: &[u8]) -> Result<(), RuntimeError> {
        while !chunk.is_empty() {
            let length = chunk.len().min(WRITE_CHUNK_BYTES - self.pending.len());
            self.pending.extend_from_slice(&chunk[..length]);
            chunk = &chunk[length..];
            if self.pending.len() == WRITE_CHUNK_BYTES {
                self.flush().await?;
            }
        }
        Ok(())
    }

    async fn flush(&mut self) -> Result<(), RuntimeError> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let expected = self.pending.len();
        let written = sqlx::query_scalar::<_, i32>("SELECT lowrite($1, $2)")
            .bind(self.fd)
            .bind(&self.pending)
            .fetch_one(&mut *self.tx)
            .await
            .map_err(schema_error)?;
        if written as usize != expected {
            return Err(protocol_error(format!(
                "large object short write: {written}/{expected}"
            )));
        }
        self.position += written as i64;
        self.pending.clear();
        Ok(())
    }

    pub fn position(&self) -> i64 {
        self.position + self.pending.len() as i64
    }

    pub fn connection(&mut self) -> &mut PgConnection {
        &mut self.tx
    }

    pub async fn commit(mut self) -> Result<(), RuntimeError> {
        self.flush().await?;
        sqlx::query_scalar::<_, i32>("SELECT lo_close($1)")
            .bind(self.fd)
            .fetch_one(&mut *self.tx)
            .await
            .map_err(schema_error)?;
        self.tx.commit().await.map_err(schema_error)
    }
}

pub struct LargeObjectReader {
    tx: Option<Transaction<'static, Postgres>>,
    fd: i32,
    remaining: i64,
}

impl LargeObjectReader {
    pub async fn open(pool: &PgPool, oid: Oid, size: i64) -> Result<Self, RuntimeError> {
        let mut tx = pool.begin().await.map_err(schema_error)?;
        let fd = sqlx::query_scalar::<_, i32>("SELECT lo_open($1, $2)")
            .bind(oid)
            .bind(INV_READ)
            .fetch_one(&mut *tx)
            .await
            .map_err(schema_error)?;
        Ok(Self {
            tx: Some(tx),
            fd,
            remaining: size,
        })
    }

    async fn next_chunk(&mut self) -> Result<Option<Bytes>, RuntimeError> {
        if self.remaining == 0 {
            self.finish().await?;
            return Ok(None);
        }
        let length = self.remaining.min(READ_CHUNK_BYTES as i64) as i32;
        let tx = self
            .tx
            .as_mut()
            .ok_or_else(|| protocol_error("large object reader is closed"))?;
        let bytes = sqlx::query_scalar::<_, Vec<u8>>("SELECT loread($1, $2)")
            .bind(self.fd)
            .bind(length)
            .fetch_one(&mut **tx)
            .await
            .map_err(schema_error)?;
        if bytes.is_empty() {
            return Err(protocol_error(format!(
                "large object ended with {} bytes remaining",
                self.remaining
            )));
        }
        self.remaining -= bytes.len() as i64;
        Ok(Some(Bytes::from(bytes)))
    }

    async fn finish(&mut self) -> Result<(), RuntimeError> {
        let Some(mut tx) = self.tx.take() else {
            return Ok(());
        };
        sqlx::query_scalar::<_, i32>("SELECT lo_close($1)")
            .bind(self.fd)
            .fetch_one(&mut *tx)
            .await
            .map_err(schema_error)?;
        tx.commit().await.map_err(schema_error)
    }

    pub fn into_body(self) -> Body {
        Body::from_stream(stream::try_unfold(self, |mut reader| async move {
            match reader.next_chunk().await? {
                Some(bytes) => Ok::<_, RuntimeError>(Some((bytes, reader))),
                None => Ok::<_, RuntimeError>(None),
            }
        }))
    }

    pub async fn read_all(mut self) -> Result<Bytes, RuntimeError> {
        let mut data = Vec::new();
        while let Some(chunk) = self.next_chunk().await? {
            data.extend_from_slice(&chunk);
        }
        Ok(Bytes::from(data))
    }
}

pub async fn checksum_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    oid: Oid,
    size: i64,
) -> Result<String, RuntimeError> {
    let fd = sqlx::query_scalar::<_, i32>("SELECT lo_open($1, $2)")
        .bind(oid)
        .bind(INV_READ)
        .fetch_one(&mut **tx)
        .await
        .map_err(schema_error)?;
    let mut remaining = size;
    let mut hasher = Sha256::new();
    while remaining > 0 {
        let length = remaining.min(READ_CHUNK_BYTES as i64) as i32;
        let bytes = sqlx::query_scalar::<_, Vec<u8>>("SELECT loread($1, $2)")
            .bind(fd)
            .bind(length)
            .fetch_one(&mut **tx)
            .await
            .map_err(schema_error)?;
        if bytes.is_empty() {
            return Err(protocol_error(format!(
                "large object ended with {remaining} bytes remaining"
            )));
        }
        remaining -= bytes.len() as i64;
        hasher.update(bytes);
    }
    sqlx::query_scalar::<_, i32>("SELECT lo_close($1)")
        .bind(fd)
        .fetch_one(&mut **tx)
        .await
        .map_err(schema_error)?;
    Ok(hex::encode(hasher.finalize()))
}
