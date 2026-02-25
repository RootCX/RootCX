use std::sync::Arc;

use async_trait::async_trait;
use axum::Router;
use axum::routing::{get, post};
use sqlx::PgPool;
use tracing::info;

use crate::RuntimeError;
use crate::auth::AuthConfig;
use crate::extensions::RuntimeExtension;
use crate::routes::SharedRuntime;

pub struct AuthExtension {
    pub config: Arc<AuthConfig>,
}

#[async_trait]
impl RuntimeExtension for AuthExtension {
    fn name(&self) -> &str {
        "auth"
    }

    async fn bootstrap(&self, pool: &PgPool) -> Result<(), RuntimeError> {
        for ddl in [
            "CREATE TABLE IF NOT EXISTS rootcx_system.users (
                id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                username       TEXT NOT NULL UNIQUE,
                email          TEXT UNIQUE,
                display_name   TEXT,
                password_hash  TEXT,
                is_system      BOOLEAN NOT NULL DEFAULT false,
                created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
                updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
            )",
            "CREATE TABLE IF NOT EXISTS rootcx_system.sessions (
                id          UUID PRIMARY KEY,
                user_id     UUID NOT NULL REFERENCES rootcx_system.users(id) ON DELETE CASCADE,
                expires_at  TIMESTAMPTZ NOT NULL,
                created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
            )",
            "CREATE INDEX IF NOT EXISTS idx_sessions_user ON rootcx_system.sessions (user_id)",
            "INSERT INTO rootcx_system.users (id, username, is_system)
             VALUES ('00000000-0000-0000-0000-000000000001', 'system', true)
             ON CONFLICT (id) DO NOTHING",
        ] {
            sqlx::query(ddl).execute(pool).await.map_err(RuntimeError::Schema)?;
        }
        info!("auth schema ready");
        Ok(())
    }

    fn routes(&self) -> Option<Router<SharedRuntime>> {
        Some(
            Router::new()
                .route("/api/v1/auth/register", post(crate::routes::auth::register))
                .route("/api/v1/auth/login", post(crate::routes::auth::login))
                .route("/api/v1/auth/refresh", post(crate::routes::auth::refresh))
                .route("/api/v1/auth/logout", post(crate::routes::auth::logout))
                .route("/api/v1/auth/me", get(crate::routes::auth::me))
                .route("/api/v1/auth/mode", get(crate::routes::auth::auth_mode))
                .route("/api/v1/users", get(crate::routes::auth::list_users))
                .layer(axum::Extension(Arc::clone(&self.config))),
        )
    }
}
