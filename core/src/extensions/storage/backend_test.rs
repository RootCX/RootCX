/// Storage backend integration tests — runs against real Postgres.
///
/// What these tests catch that unit tests cannot:
/// - app_id scoping in SQL (cross-app isolation)
/// - BYTEA round-trip (data integrity through Postgres TOAST)
/// - DELETE actually removes (no phantom files)
#[cfg(test)]
mod tests {
    use sqlx::PgPool;
    use uuid::Uuid;

    use crate::extensions::storage::backend::{PostgresBackend, StorageBackend};

    async fn pool() -> PgPool {
        let url = std::env::var("TEST_DATABASE_URL")
            .unwrap_or_else(|_| "postgres://rootcx:rootcx@localhost:5480/rootcx".into());
        let pool = PgPool::connect(&url).await.expect("connect to test DB");

        // Ensure schema + table exist (idempotent, no race)
        let _ = sqlx::query("CREATE SCHEMA IF NOT EXISTS rootcx_system").execute(&pool).await;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS rootcx_system.files (
                id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                app_id TEXT NOT NULL,
                name TEXT NOT NULL,
                content_type TEXT NOT NULL DEFAULT 'application/octet-stream',
                size BIGINT NOT NULL,
                content BYTEA NOT NULL,
                uploaded_by UUID,
                created_at TIMESTAMPTZ NOT NULL DEFAULT now()
            )"
        ).execute(&pool).await.ok(); // ignore if exists (concurrent test race)

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
    async fn put_and_get_roundtrip() {
        let pool = pool().await;
        let id = Uuid::new_v4();
        let data = b"<?xml version=\"1.0\"?><Invoice><ID>TEST</ID></Invoice>";

        PostgresBackend.put(&pool, id, "peppol", "invoice.xml", "application/xml", data, None).await.unwrap();
        let obj = PostgresBackend.get(&pool, id, "peppol").await.unwrap();

        assert_eq!(&obj.content[..], data);
        assert_eq!(obj.name, "invoice.xml");
        assert_eq!(obj.content_type, "application/xml");
        assert_eq!(obj.size, data.len() as i64);

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
    async fn delete_removes_file() {
        let pool = pool().await;
        let id = Uuid::new_v4();

        PostgresBackend.put(&pool, id, "myapp", "tmp.txt", "text/plain", b"hello", None).await.unwrap();
        PostgresBackend.delete(&pool, id, "myapp").await.unwrap();

        let result = PostgresBackend.get(&pool, id, "myapp").await;
        assert!(result.is_err(), "file should be gone after delete");
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

    // ── Nonce → upload → DB: full chain without IPC/HTTP ────────────────

    #[tokio::test]
    async fn nonce_scoped_upload_then_get() {
        // Simulates the full chain: nonce created for app_a, file stored, retrievable only by app_a
        let pool = pool().await;
        let mut store = crate::extensions::storage::nonce::NonceStore::default();

        let nonce = store.create("app_a", "invoice.xml", "application/xml", 0);
        let entry = store.consume(&nonce).expect("nonce valid");

        let id = Uuid::new_v4();
        let data = b"<Invoice>test</Invoice>";
        PostgresBackend.put(&pool, id, &entry.app_id, &entry.name, &entry.content_type, data, None).await.unwrap();

        // app_a can read
        let obj = PostgresBackend.get(&pool, id, "app_a").await.unwrap();
        assert_eq!(&obj.content[..], data);

        // app_b cannot
        assert!(PostgresBackend.get(&pool, id, "app_b").await.is_err());

        // nonce is consumed — second use fails
        assert!(store.consume(&nonce).is_none());

        cleanup(&pool, &[id]).await;
    }

    #[tokio::test]
    async fn large_file_roundtrip() {
        let pool = pool().await;
        let id = Uuid::new_v4();
        let data = vec![0x42u8; 2 * 1024 * 1024]; // 2 MiB — exceeds IPC MAX_LINE_LENGTH

        PostgresBackend.put(&pool, id, "peppol", "big.xml", "application/xml", &data, None).await.unwrap();
        let obj = PostgresBackend.get(&pool, id, "peppol").await.unwrap();

        assert_eq!(obj.content.len(), data.len());
        assert_eq!(&obj.content[..], &data[..]);

        cleanup(&pool, &[id]).await;
    }
}
