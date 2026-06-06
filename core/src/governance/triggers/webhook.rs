use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::RuntimeError;
use crate::auth::secure_tokens;

fn err(e: sqlx::Error) -> RuntimeError {
    RuntimeError::Schema(e)
}

const PREFIX_LEN: usize = 12;

fn token_prefix(raw: &str) -> String {
    raw.chars().take(PREFIX_LEN).collect()
}

pub struct WebhookCredentials {
    pub id: Uuid,
    pub token: String,
    pub signing_secret: String,
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

    // Migration: prefix + hash columns
    sqlx::query("ALTER TABLE rootcx_system.webhooks ADD COLUMN IF NOT EXISTS prefix TEXT")
        .execute(pool).await.map_err(err)?;
    sqlx::query("ALTER TABLE rootcx_system.webhooks ADD COLUMN IF NOT EXISTS token_hash BYTEA")
        .execute(pool).await.map_err(err)?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_webhooks_prefix ON rootcx_system.webhooks (prefix) WHERE prefix IS NOT NULL")
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
    pub prefix: Option<String>,
    pub token_hash: Option<Vec<u8>>,
    pub created_by: Option<Uuid>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

pub async fn sync_webhooks(
    pool: &PgPool,
    app_id: &str,
    webhooks: &[rootcx_types::WebhookDefinition],
    created_by: Option<Uuid>,
    secrets: &crate::secrets::SecretManager,
) -> Result<Vec<WebhookCredentials>, RuntimeError> {
    let names: Vec<&str> = webhooks.iter().map(|w| w.name()).collect();

    sqlx::query(
        "DELETE FROM rootcx_system.webhooks WHERE app_id = $1 AND name != ALL($2)"
    )
    .bind(app_id)
    .bind(&names)
    .execute(pool)
    .await
    .map_err(err)?;

    let mut credentials = Vec::new();

    for wh in webhooks {
        let token = secure_tokens::generate();
        let signing_secret = secure_tokens::generate();
        let prefix = token_prefix(&token);
        let token_hash = secure_tokens::hash(&token).to_vec();

        let (id,): (Uuid,) = sqlx::query_as(r#"
            INSERT INTO rootcx_system.webhooks (app_id, name, method, token, prefix, token_hash, created_by)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT (app_id, name) DO UPDATE SET method = EXCLUDED.method, prefix = EXCLUDED.prefix, token_hash = EXCLUDED.token_hash, token = EXCLUDED.token, created_by = COALESCE(EXCLUDED.created_by, rootcx_system.webhooks.created_by)
            RETURNING id
        "#)
        .bind(app_id)
        .bind(wh.name())
        .bind(wh.method())
        .bind(&token) // kept for backwards compat during migration window
        .bind(&prefix)
        .bind(&token_hash)
        .bind(created_by)
        .fetch_one(pool)
        .await
        .map_err(err)?;

        // Store signing_secret encrypted via SecretManager
        let scope = format!("webhook:{id}");
        secrets.set(pool, &scope, "signing_secret", &signing_secret)
            .await
            .map_err(|e| RuntimeError::Secret(e.to_string()))?;

        if let Some(owner) = created_by {
            let agent_uid = crate::extensions::agents::agent_user_id(app_id);
            let _ = crate::governance::delegation::create(pool, owner, agent_uid, "webhook", Some(id)).await;
        }

        credentials.push(WebhookCredentials { id, token, signing_secret });
    }

    Ok(credentials)
}

pub async fn list_webhooks(pool: &PgPool, app_id: &str) -> Result<Vec<WebhookRow>, RuntimeError> {
    sqlx::query_as::<_, WebhookRow>(
        "SELECT id, app_id, name, method, token, prefix, token_hash, created_by, created_at FROM rootcx_system.webhooks WHERE app_id = $1 ORDER BY name"
    )
    .bind(app_id)
    .fetch_all(pool)
    .await
    .map_err(err)
}

/// Lookup by prefix + constant-time hash verification.
/// Falls back to legacy plaintext token match for pre-migration rows.
pub async fn lookup_token(pool: &PgPool, token: &str) -> Result<Option<WebhookRow>, RuntimeError> {
    let prefix = token_prefix(token);
    let candidates: Vec<WebhookRow> = sqlx::query_as(
        "SELECT id, app_id, name, method, token, prefix, token_hash, created_by, created_at FROM rootcx_system.webhooks WHERE prefix = $1"
    )
    .bind(&prefix)
    .fetch_all(pool)
    .await
    .map_err(err)?;

    let candidate_hash = secure_tokens::hash(token);
    for row in candidates {
        if let Some(ref stored_hash) = row.token_hash {
            if secure_tokens::verify(stored_hash, &candidate_hash) {
                return Ok(Some(row));
            }
        }
    }

    // Legacy fallback: direct plaintext match for unmigrated rows
    let legacy = sqlx::query_as::<_, WebhookRow>(
        "SELECT id, app_id, name, method, token, prefix, token_hash, created_by, created_at FROM rootcx_system.webhooks WHERE token = $1 AND prefix IS NULL"
    )
    .bind(token)
    .fetch_optional(pool)
    .await
    .map_err(err)?;

    Ok(legacy)
}

/// Retrieve the encrypted signing_secret for a webhook.
pub async fn get_signing_secret(
    pool: &PgPool,
    secrets: &crate::secrets::SecretManager,
    webhook_id: Uuid,
) -> Result<Option<String>, RuntimeError> {
    let scope = format!("webhook:{webhook_id}");
    secrets.get(pool, &scope, "signing_secret")
        .await
        .map_err(|e| RuntimeError::Secret(e.to_string()))
}

/// Verify HMAC signature: HMAC-SHA256(signing_secret, timestamp + "." + body)
pub fn verify_hmac(signing_secret: &str, timestamp: &str, body: &[u8], signature: &str) -> bool {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    let sig_hex = signature.strip_prefix("sha256=").unwrap_or(signature);
    let Ok(expected) = hex::decode(sig_hex) else { return false };

    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(signing_secret.as_bytes()) else {
        return false;
    };
    mac.update(timestamp.as_bytes());
    mac.update(b".");
    mac.update(body);

    mac.verify_slice(&expected).is_ok()
}

/// Reject timestamps older than max_age_secs (replay protection).
pub fn is_timestamp_fresh(timestamp: &str, max_age_secs: u64) -> bool {
    let Ok(ts) = timestamp.parse::<i64>() else { return false };
    let now = chrono::Utc::now().timestamp();
    let age = now.saturating_sub(ts);
    age >= 0 && (age as u64) <= max_age_secs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_valid_signature() {
        use hmac::{Hmac, Mac};
        use sha2::Sha256;

        let secret = "test-secret-key";
        let timestamp = "1700000000";
        let body = b"hello world";

        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(timestamp.as_bytes());
        mac.update(b".");
        mac.update(body);
        let sig = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));

        assert!(verify_hmac(secret, timestamp, body, &sig));
    }

    #[test]
    fn hmac_invalid_signature() {
        assert!(!verify_hmac(
            "test-secret-key",
            "1700000000",
            b"hello world",
            "sha256=deadbeef0000000000000000000000000000000000000000000000000000abcd",
        ));
    }

    #[test]
    fn hmac_tampered_body() {
        use hmac::{Hmac, Mac};
        use sha2::Sha256;

        let secret = "test-secret-key";
        let timestamp = "1700000000";

        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(timestamp.as_bytes());
        mac.update(b".");
        mac.update(b"original body");
        let sig = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));

        assert!(!verify_hmac(secret, timestamp, b"tampered body", &sig));
    }

    #[test]
    fn timestamp_fresh() {
        let now = chrono::Utc::now().timestamp().to_string();
        assert!(is_timestamp_fresh(&now, 300));
    }

    #[test]
    fn timestamp_stale() {
        let old = (chrono::Utc::now().timestamp() - 600).to_string();
        assert!(!is_timestamp_fresh(&old, 300));
    }

    #[test]
    fn timestamp_invalid() {
        assert!(!is_timestamp_fresh("not-a-number", 300));
    }

    #[test]
    fn prefix_extraction() {
        let token = "ABCDEFGHIJKLrest-of-token";
        assert_eq!(token_prefix(token), "ABCDEFGHIJKL");
    }

}
