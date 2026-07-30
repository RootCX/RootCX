use async_trait::async_trait;
use axum::{Json, Router, extract::State, routing::get};
use serde::Serialize;
use sqlx::PgPool;

use super::RuntimeExtension;
use crate::{RuntimeError, api_error::ApiError, auth::identity::Identity, routes::SharedRuntime};

pub struct OnboardingExtension;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OnboardingStatus {
    pub connected: bool,
    pub first_app_deployed: bool,
    pub first_app_id: Option<String>,
    pub first_app_url: Option<String>,
    pub deployed_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[async_trait]
impl RuntimeExtension for OnboardingExtension {
    fn name(&self) -> &str {
        "onboarding"
    }

    async fn bootstrap(&self, pool: &PgPool) -> Result<(), RuntimeError> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS rootcx_system.onboarding_activation (
                singleton       BOOLEAN PRIMARY KEY DEFAULT true CHECK (singleton),
                connected_by    UUID REFERENCES rootcx_system.users(id) ON DELETE SET NULL,
                connected_at    TIMESTAMPTZ,
                first_app_id    TEXT,
                deployed_by     UUID REFERENCES rootcx_system.users(id) ON DELETE SET NULL,
                deployed_at     TIMESTAMPTZ
            )",
        )
        .execute(pool)
        .await
        .map_err(RuntimeError::Schema)?;
        for ddl in [
            "ALTER TABLE rootcx_system.onboarding_activation ADD COLUMN IF NOT EXISTS connected_by UUID REFERENCES rootcx_system.users(id) ON DELETE SET NULL",
            "ALTER TABLE rootcx_system.onboarding_activation ADD COLUMN IF NOT EXISTS connected_at TIMESTAMPTZ",
            "ALTER TABLE rootcx_system.onboarding_activation ALTER COLUMN first_app_id DROP NOT NULL",
            "ALTER TABLE rootcx_system.onboarding_activation ALTER COLUMN deployed_by DROP NOT NULL",
            "ALTER TABLE rootcx_system.onboarding_activation ALTER COLUMN deployed_at DROP NOT NULL",
        ] {
            sqlx::query(ddl)
                .execute(pool)
                .await
                .map_err(RuntimeError::Schema)?;
        }
        Ok(())
    }

    fn routes(&self) -> Option<Router<SharedRuntime>> {
        Some(Router::new().route("/api/v1/onboarding/status", get(status)))
    }
}

pub(crate) async fn record_first_app(
    pool: &PgPool,
    user_id: uuid::Uuid,
    app_id: &str,
) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO rootcx_system.onboarding_activation
            (singleton, first_app_id, deployed_by, deployed_at)
         VALUES (true, $1, $2, now())
         ON CONFLICT (singleton) DO UPDATE SET
            first_app_id = COALESCE(rootcx_system.onboarding_activation.first_app_id, EXCLUDED.first_app_id),
            deployed_by = COALESCE(rootcx_system.onboarding_activation.deployed_by, EXCLUDED.deployed_by),
            deployed_at = COALESCE(rootcx_system.onboarding_activation.deployed_at, EXCLUDED.deployed_at)",
    )
    .bind(app_id)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub(crate) async fn record_connection(pool: &PgPool, user_id: uuid::Uuid) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO rootcx_system.onboarding_activation (singleton, connected_by, connected_at)
         VALUES (true, $1, now())
         ON CONFLICT (singleton) DO UPDATE SET
            connected_by = COALESCE(rootcx_system.onboarding_activation.connected_by, EXCLUDED.connected_by),
            connected_at = COALESCE(rootcx_system.onboarding_activation.connected_at, EXCLUDED.connected_at)",
    )
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub(crate) async fn read_status(pool: &PgPool) -> Result<OnboardingStatus, ApiError> {
    let row: Option<(Option<chrono::DateTime<chrono::Utc>>, Option<String>, Option<chrono::DateTime<chrono::Utc>>)> = sqlx::query_as(
        "SELECT connected_at, first_app_id, deployed_at FROM rootcx_system.onboarding_activation WHERE singleton = true",
    )
    .fetch_optional(pool)
    .await?;
    let public_url = std::env::var("ROOTCX_PUBLIC_URL").ok();
    let (connected_at, first_app_id, deployed_at) = row.unwrap_or((None, None, None));
    let first_app_url = first_app_id.as_ref().map(|id| {
        let path = format!("/apps/{id}/");
        public_url
            .as_ref()
            .map(|base| format!("{}{}", base.trim_end_matches('/'), path))
            .unwrap_or(path)
    });
    Ok(OnboardingStatus {
        connected: connected_at.is_some(),
        first_app_deployed: first_app_id.is_some(),
        first_app_id,
        first_app_url,
        deployed_at,
    })
}

async fn status(
    _identity: Identity,
    State(runtime): State<SharedRuntime>,
) -> Result<Json<OnboardingStatus>, ApiError> {
    Ok(Json(read_status(runtime.pool()).await?))
}
