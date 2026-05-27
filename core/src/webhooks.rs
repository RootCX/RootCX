use rand::Rng;
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::RuntimeError;

fn err(e: sqlx::Error) -> RuntimeError {
    RuntimeError::Schema(e)
}

fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill(&mut bytes);
    hex::encode(bytes)
}

pub async fn bootstrap(pool: &PgPool) -> Result<(), RuntimeError> {
    sqlx::query(r#"
        CREATE TABLE IF NOT EXISTS rootcx_system.webhooks (
            id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            app_id     TEXT NOT NULL REFERENCES rootcx_system.apps(id) ON DELETE CASCADE,
            name       TEXT NOT NULL,
            method     TEXT NOT NULL,
            token      TEXT NOT NULL UNIQUE,
            created_by UUID,
            created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
            UNIQUE (app_id, name)
        )
    "#).execute(pool).await.map_err(err)?;

    sqlx::query("ALTER TABLE rootcx_system.webhooks ADD COLUMN IF NOT EXISTS created_by UUID")
        .execute(pool).await.map_err(err)?;

    Ok(())
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct WebhookRow {
    pub id: Uuid,
    pub app_id: String,
    pub name: String,
    pub method: String,
    pub token: String,
    pub created_by: Option<Uuid>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

pub async fn sync_webhooks(
    pool: &PgPool,
    app_id: &str,
    webhooks: &[rootcx_types::WebhookDefinition],
    created_by: Option<Uuid>,
) -> Result<(), RuntimeError> {
    let names: Vec<&str> = webhooks.iter().map(|w| w.name()).collect();

    sqlx::query(
        "DELETE FROM rootcx_system.webhooks WHERE app_id = $1 AND name != ALL($2)"
    )
    .bind(app_id)
    .bind(&names)
    .execute(pool)
    .await
    .map_err(err)?;

    for wh in webhooks {
        let (id,): (Uuid,) = sqlx::query_as(r#"
            INSERT INTO rootcx_system.webhooks (app_id, name, method, token, created_by)
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (app_id, name) DO UPDATE SET method = EXCLUDED.method, created_by = COALESCE(EXCLUDED.created_by, rootcx_system.webhooks.created_by)
            RETURNING id
        "#)
        .bind(app_id)
        .bind(wh.name())
        .bind(wh.method())
        .bind(generate_token())
        .bind(created_by)
        .fetch_one(pool)
        .await
        .map_err(err)?;

        if let Some(owner) = created_by {
            let agent_uid = crate::extensions::agents::agent_user_id(app_id);
            let _ = crate::delegations::create(pool, owner, agent_uid, "webhook", Some(id)).await;
        }
    }

    Ok(())
}

pub async fn list_webhooks(pool: &PgPool, app_id: &str) -> Result<Vec<WebhookRow>, RuntimeError> {
    sqlx::query_as::<_, WebhookRow>(
        "SELECT id, app_id, name, method, token, created_by, created_at FROM rootcx_system.webhooks WHERE app_id = $1 ORDER BY name"
    )
    .bind(app_id)
    .fetch_all(pool)
    .await
    .map_err(err)
}

pub async fn lookup_token(pool: &PgPool, token: &str) -> Result<Option<WebhookRow>, RuntimeError> {
    sqlx::query_as::<_, WebhookRow>(
        "SELECT id, app_id, name, method, token, created_by, created_at FROM rootcx_system.webhooks WHERE token = $1"
    )
    .bind(token)
    .fetch_optional(pool)
    .await
    .map_err(err)
}
