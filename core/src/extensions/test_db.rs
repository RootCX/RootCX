//! Shared bootstrap for library DB tests, compiled only under cfg(test).

use std::sync::Arc;
use std::time::Duration;

use jsonwebtoken::{DecodingKey, EncodingKey};
use sqlx::PgPool;

use crate::extensions::RuntimeExtension;

// Share completion only, never a pool: tokio tests use separate runtimes, and
// shutting down the initializing test's runtime must not break later tests.
static SCHEMA: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();

pub(crate) async fn pool() -> PgPool {
    let url = std::env::var("TEST_DATABASE_URL")
        .expect("TEST_DATABASE_URL is required; run make core-unit");
    let pool = PgPool::connect(&url).await.expect("connect to test DB");
    SCHEMA
        .get_or_init(|| async {
            // Match Runtime::boot's dependency order, then initialize only the
            // extensions exercised by these library tests.
            crate::schema::bootstrap(&pool)
                .await
                .expect("bootstrap core schema");
            crate::extensions::auth::bootstrap_users_table(&pool)
                .await
                .expect("bootstrap users table");
            sqlx::migrate!("./migrations")
                .run(&pool)
                .await
                .expect("run core migrations");
            crate::extensions::audit::AuditExtension
                .bootstrap(&pool)
                .await
                .expect("bootstrap audit");
            let secret = b"platform-storage-test-secret-32b";
            crate::extensions::auth::AuthExtension {
                config: Arc::new(crate::auth::AuthConfig {
                    encoding_key: EncodingKey::from_secret(secret),
                    decoding_key: DecodingKey::from_secret(secret),
                    access_ttl: Duration::from_secs(900),
                    refresh_ttl: Duration::from_secs(3600),
                }),
            }
            .bootstrap(&pool)
            .await
            .expect("bootstrap auth");
            crate::extensions::rbac::RbacExtension
                .bootstrap(&pool)
                .await
                .expect("bootstrap RBAC");
            crate::extensions::integrations::connections::bootstrap(&pool)
                .await
                .expect("bootstrap integration connections");
            crate::extensions::storage::StorageExtension
                .bootstrap(&pool)
                .await
                .expect("bootstrap storage");
            crate::extensions::platform_storage::PlatformStorageExtension
                .bootstrap(&pool)
                .await
                .expect("bootstrap platform storage");
        })
        .await;
    pool
}
