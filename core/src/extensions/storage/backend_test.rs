/// Storage backend integration tests — runs against real Postgres.
///
/// What these tests catch that unit tests cannot:
/// - app_id scoping in SQL (cross-app isolation)
/// - Large Object round-trip across multiple writes
/// - legacy BYTEA compatibility
/// - DELETE unlinks the Large Object
#[cfg(test)]
mod tests {
    use sha2::Digest;
    use sqlx::postgres::types::Oid;
    use sqlx::PgPool;
    use tokio::sync::Mutex;
    use uuid::Uuid;

    use crate::extensions::storage::backend::{PostgresBackend, StorageBackend};
    use crate::extensions::storage::StorageExtension;
    use crate::extensions::RuntimeExtension;

    static BOOTSTRAP: Mutex<()> = Mutex::const_new(());

    async fn pool() -> PgPool {
        let url = std::env::var("TEST_DATABASE_URL")
            .unwrap_or_else(|_| "postgres://rootcx:rootcx@localhost:5480/rootcx".into());
        let pool = PgPool::connect(&url).await.expect("connect to test DB");
        let _bootstrap = BOOTSTRAP.lock().await;
        sqlx::query("CREATE SCHEMA IF NOT EXISTS rootcx_system").execute(&pool).await.unwrap();
        StorageExtension.bootstrap(&pool).await.expect("bootstrap storage");
        pool
    }

    async fn cleanup(pool: &PgPool, ids: &[Uuid]) {
        for id in ids {
            let _ = sqlx::query("DELETE FROM rootcx_system.files WHERE id = $1")
                .bind(id)
                .execute(pool)
                .await;
        }
    }

    #[tokio::test]
    async fn new_files_roundtrip_through_large_objects() {
        let pool = pool().await;
        let id = Uuid::new_v4();
        let data = b"<?xml version=\"1.0\"?><Invoice><ID>TEST</ID></Invoice>";

        PostgresBackend.put(&pool, id, "peppol", "invoice.xml", "application/xml", data, None).await.unwrap();
        let obj = PostgresBackend.get(&pool, id, "peppol").await.unwrap();

        assert_eq!(&obj.content[..], data);
        assert_eq!(obj.name, "invoice.xml");
        assert_eq!(obj.content_type, "application/xml");
        assert_eq!(obj.size, data.len() as i64);
        let storage: (bool, Option<Oid>) = sqlx::query_as(
            "SELECT content IS NULL, content_oid FROM rootcx_system.files WHERE id = $1",
        ).bind(id).fetch_one(&pool).await.unwrap();
        assert!(storage.0, "new files must not use inline BYTEA");
        assert!(storage.1.is_some(), "new files must reference a Large Object");

        cleanup(&pool, &[id]).await;
    }

    #[tokio::test]
    async fn get_wrong_app_id_returns_not_found() {
        let pool = pool().await;
        let id = Uuid::new_v4();

        PostgresBackend.put(&pool, id, "app_a", "secret.pdf", "application/pdf", b"secret", None).await.unwrap();

        let result = PostgresBackend.get(&pool, id, "app_b").await;
        assert!(result.is_err(), "app_b must not access app_a's file");

        assert!(PostgresBackend.get(&pool, id, "app_a").await.is_ok());

        cleanup(&pool, &[id]).await;
    }

    #[tokio::test]
    async fn delete_wrong_app_id_returns_not_found() {
        let pool = pool().await;
        let id = Uuid::new_v4();

        PostgresBackend.put(&pool, id, "app_a", "doc.xml", "text/xml", b"<doc/>", None).await.unwrap();

        let result = PostgresBackend.delete(&pool, id, "app_b").await;
        assert!(result.is_err(), "app_b must not delete app_a's file");

        assert!(PostgresBackend.get(&pool, id, "app_a").await.is_ok(), "file should still exist");

        cleanup(&pool, &[id]).await;
    }

    #[tokio::test]
    async fn delete_unlinks_large_object() {
        let pool = pool().await;
        let id = Uuid::new_v4();

        PostgresBackend.put(&pool, id, "myapp", "tmp.txt", "text/plain", b"hello", None).await.unwrap();
        let oid: Oid = sqlx::query_scalar("SELECT content_oid FROM rootcx_system.files WHERE id = $1")
            .bind(id).fetch_one(&pool).await.unwrap();
        PostgresBackend.delete(&pool, id, "myapp").await.unwrap();

        let result = PostgresBackend.get(&pool, id, "myapp").await;
        assert!(result.is_err(), "file should be gone after delete");
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM pg_largeobject_metadata WHERE oid = $1)",
        ).bind(oid).fetch_one(&pool).await.unwrap();
        assert!(!exists, "deleting metadata must unlink the Large Object");
    }

    #[tokio::test]
    async fn get_nonexistent_returns_not_found() {
        let pool = pool().await;
        assert!(PostgresBackend.get(&pool, Uuid::new_v4(), "any").await.is_err());
    }

    #[tokio::test]
    async fn delete_nonexistent_returns_not_found() {
        let pool = pool().await;
        assert!(PostgresBackend.delete(&pool, Uuid::new_v4(), "any").await.is_err());
    }

    #[tokio::test]
    async fn buffered_access_rejects_large_objects() {
        let pool = pool().await;
        let id = Uuid::new_v4();
        PostgresBackend.put(&pool, id, "imports", "large.xlsx", "application/octet-stream", b"x", None).await.unwrap();
        sqlx::query("UPDATE rootcx_system.files SET size = 67108865 WHERE id = $1")
            .bind(id).execute(&pool).await.unwrap();

        let error = PostgresBackend.get(&pool, id, "imports").await.err().expect("buffered access must fail");
        assert!(error.to_string().contains("too large for buffered access"));
        cleanup(&pool, &[id]).await;
    }

    #[tokio::test]
    async fn writer_preserves_content_across_internal_chunk_boundaries() {
        let pool = pool().await;
        let id = Uuid::new_v4();
        let chunks = [vec![0x11; 700_000], vec![0x22; 900_000], vec![0x33; 500_000]];
        let expected: Vec<u8> = chunks.iter().flatten().copied().collect();
        let mut writer = PostgresBackend.begin(&pool).await.unwrap();
        for chunk in &chunks {
            writer.write(chunk).await.unwrap();
        }
        let upload = PostgresBackend
            .finish(writer, id, "imports", "catalog.xlsx", "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", None)
            .await
            .unwrap();

        assert_eq!(upload.size, expected.len() as i64);
        assert_eq!(upload.checksum, hex::encode(sha2::Sha256::digest(&expected)));
        let object = PostgresBackend.get(&pool, id, "imports").await.unwrap();
        assert_eq!(&object.content[..], &expected);
        cleanup(&pool, &[id]).await;
    }

    #[tokio::test]
    async fn legacy_bytea_remains_readable() {
        let pool = pool().await;
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO rootcx_system.files (id, app_id, name, content_type, size, content) \
             VALUES ($1, 'legacy', 'before.txt', 'text/plain', 6, 'before')",
        ).bind(id).execute(&pool).await.unwrap();

        let object = PostgresBackend.get(&pool, id, "legacy").await.unwrap();
        assert_eq!(&object.content[..], b"before");
        cleanup(&pool, &[id]).await;
    }
}
