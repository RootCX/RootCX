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
                first_app_id    TEXT,
                deployed_by     UUID REFERENCES rootcx_system.users(id) ON DELETE SET NULL,
                deployed_at     TIMESTAMPTZ
            )",
        )
        .execute(pool)
        .await
        .map_err(RuntimeError::Schema)?;
        // Connecting an AI environment is a per-person act: every member pairs
        // their own Claude Code or ChatGPT against this workspace. The legacy
        // singleton recorded only the first person to do it, which made every
        // other member look connected. Rows here, one per user, are the source
        // of truth; the singleton keeps only workspace-wide milestones.
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS rootcx_system.onboarding_connections (
                user_id         UUID PRIMARY KEY REFERENCES rootcx_system.users(id) ON DELETE CASCADE,
                connected_at    TIMESTAMPTZ NOT NULL DEFAULT now()
            )",
        )
        .execute(pool)
        .await
        .map_err(RuntimeError::Schema)?;
        for ddl in [
            "ALTER TABLE rootcx_system.onboarding_activation ALTER COLUMN first_app_id DROP NOT NULL",
            "ALTER TABLE rootcx_system.onboarding_activation ALTER COLUMN deployed_by DROP NOT NULL",
            "ALTER TABLE rootcx_system.onboarding_activation ALTER COLUMN deployed_at DROP NOT NULL",
            // Carry the one legacy connection over, then retire its columns so
            // there is a single source of truth.
            "DO $mig$
             BEGIN
                IF EXISTS (SELECT 1 FROM information_schema.columns
                           WHERE table_schema = 'rootcx_system'
                             AND table_name = 'onboarding_activation'
                             AND column_name = 'connected_by') THEN
                    INSERT INTO rootcx_system.onboarding_connections (user_id, connected_at)
                    SELECT connected_by, COALESCE(connected_at, now())
                    FROM rootcx_system.onboarding_activation
                    WHERE connected_by IS NOT NULL
                    ON CONFLICT (user_id) DO NOTHING;
                    ALTER TABLE rootcx_system.onboarding_activation DROP COLUMN connected_by;
                    ALTER TABLE rootcx_system.onboarding_activation DROP COLUMN connected_at;
                END IF;
             END
             $mig$",
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
        "INSERT INTO rootcx_system.onboarding_connections (user_id, connected_at)
         VALUES ($1, now())
         ON CONFLICT (user_id) DO NOTHING",
    )
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Activation as seen by one member: `connected` is that person's own pairing,
/// while the first deployed app is a milestone the whole workspace shares.
pub(crate) async fn read_status(
    pool: &PgPool,
    user_id: uuid::Uuid,
) -> Result<OnboardingStatus, ApiError> {
    let connected_at: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
        "SELECT connected_at FROM rootcx_system.onboarding_connections WHERE user_id = $1",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    let row: Option<(Option<String>, Option<chrono::DateTime<chrono::Utc>>)> = sqlx::query_as(
        "SELECT first_app_id, deployed_at FROM rootcx_system.onboarding_activation WHERE singleton = true",
    )
    .fetch_optional(pool)
    .await?;
    let public_url = std::env::var("ROOTCX_PUBLIC_URL").ok();
    let (first_app_id, deployed_at) = row.unwrap_or((None, None));
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
    identity: Identity,
    State(runtime): State<SharedRuntime>,
) -> Result<Json<OnboardingStatus>, ApiError> {
    Ok(Json(read_status(runtime.pool(), identity.user_id).await?))
}
