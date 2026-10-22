use axum::Json;
use axum::extract::{Path, State};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use uuid::Uuid;

use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::routes::{self, SharedRuntime};

/// Binding-scope sentinel in app_integrations.user_id: '' = app-wide default.
pub(crate) const APP_WIDE: &str = "";
const SCOPE_USER: &str = "user";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Connection {
    pub id: String,
    pub integration_id: String,
    pub user_id: String,
    pub label: Option<String>,
    pub created_at: String,
    pub status: String,
    pub is_default: bool,
}

pub async fn bootstrap(pool: &sqlx::PgPool) -> Result<(), crate::RuntimeError> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS rootcx_system.integration_connections (
            id TEXT PRIMARY KEY,
            integration_id TEXT NOT NULL,
            user_id TEXT NOT NULL,
            label TEXT,
            kind TEXT NOT NULL DEFAULT 'direct',
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        )"
    ).execute(pool).await.map_err(crate::RuntimeError::Schema)?;

    sqlx::query(
        "ALTER TABLE rootcx_system.integration_connections
         ADD COLUMN IF NOT EXISTS kind TEXT NOT NULL DEFAULT 'direct'"
    ).execute(pool).await.map_err(crate::RuntimeError::Schema)?;

    // Health columns: a connection whose provider credentials have been rejected
    // (e.g. Google invalid_grant) is flagged 'dead' so status() surfaces the
    // failure instead of reporting it connected. Additive + idempotent.
    //
    // config_id: which named provider config (OAuth client) authorized this
    // connection. NULL = the integration's default config (legacy '_platform'
    // secrets). A connection refreshes with the client that issued its token, so
    // the config is pinned per connection, not per integration.
    for ddl in [
        "ALTER TABLE rootcx_system.integration_connections ADD COLUMN IF NOT EXISTS status TEXT NOT NULL DEFAULT 'active'",
        "ALTER TABLE rootcx_system.integration_connections ADD COLUMN IF NOT EXISTS last_error TEXT",
        "ALTER TABLE rootcx_system.integration_connections ADD COLUMN IF NOT EXISTS last_error_at TIMESTAMPTZ",
        "ALTER TABLE rootcx_system.integration_connections ADD COLUMN IF NOT EXISTS config_id TEXT",
    ] {
        sqlx::query(ddl).execute(pool).await.map_err(crate::RuntimeError::Schema)?;
    }

    // Named provider configs: additional OAuth clients for one integration (e.g.
    // a second Google project). Each holds its own client_id/secret as secrets
    // scoped under its id. The default config stays the legacy '_platform' scope
    // (a NULL config_id on a connection), so existing installs are untouched.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS rootcx_system.integration_configs (
            id TEXT PRIMARY KEY,
            integration_id TEXT NOT NULL,
            label TEXT NOT NULL,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            UNIQUE (integration_id, label)
        )"
    ).execute(pool).await.map_err(crate::RuntimeError::Schema)?;

    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_ic_user_integration
         ON rootcx_system.integration_connections (user_id, integration_id)"
    ).execute(pool).await.map_err(crate::RuntimeError::Schema)?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS rootcx_system.app_integrations (
            app_id TEXT NOT NULL,
            integration_id TEXT NOT NULL,
            connection_id TEXT,
            enabled BOOLEAN NOT NULL DEFAULT true,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            PRIMARY KEY (app_id, integration_id)
        )"
    ).execute(pool).await.map_err(crate::RuntimeError::Schema)?;

    sqlx::query(
        "ALTER TABLE rootcx_system.app_integrations
         ADD COLUMN IF NOT EXISTS connection_id TEXT"
    ).execute(pool).await.map_err(crate::RuntimeError::Schema)?;

    // Bindings are scoped per (app × user): '' = app-wide default, a user id =
    // that user's own connection choice for this app. PK → unique triple.
    // A user-scoped row without a connection would grant consent yet resolve
    // through someone else's app-wide connection — forbidden at the data layer.
    for ddl in [
        "ALTER TABLE rootcx_system.app_integrations ADD COLUMN IF NOT EXISTS user_id TEXT NOT NULL DEFAULT ''",
        "ALTER TABLE rootcx_system.app_integrations DROP CONSTRAINT IF EXISTS app_integrations_pkey",
        "CREATE UNIQUE INDEX IF NOT EXISTS uq_app_integrations_scope
         ON rootcx_system.app_integrations (app_id, integration_id, user_id)",
        "DO $$ BEGIN
            IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'chk_user_binding_has_connection') THEN
                ALTER TABLE rootcx_system.app_integrations
                    ADD CONSTRAINT chk_user_binding_has_connection
                    CHECK (user_id = '' OR connection_id IS NOT NULL);
            END IF;
        END $$",
    ] {
        sqlx::query(ddl).execute(pool).await.map_err(crate::RuntimeError::Schema)?;
    }

    // A user's default connection serves every app that has no account choice of
    // its own. At most one per (integration × user); it lives on the connection
    // row, so deleting the connection can never leave a default behind.
    //
    // A binding exists only while its app and its connection do: removing either
    // removes the binding in the same statement, whichever path removes it.
    // Rows orphaned before these keys existed are dropped once, when they are added.
    for ddl in [
        "ALTER TABLE rootcx_system.integration_connections ADD COLUMN IF NOT EXISTS is_default BOOLEAN NOT NULL DEFAULT false",
        "CREATE UNIQUE INDEX IF NOT EXISTS uq_integration_default_connection
         ON rootcx_system.integration_connections (integration_id, user_id) WHERE is_default",
        "CREATE INDEX IF NOT EXISTS idx_app_integrations_connection
         ON rootcx_system.app_integrations (connection_id) WHERE connection_id IS NOT NULL",
        "DO $$ BEGIN
            IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'fk_app_integrations_app') THEN
                DELETE FROM rootcx_system.app_integrations b
                    WHERE NOT EXISTS (SELECT 1 FROM rootcx_system.apps a WHERE a.id = b.app_id);
                ALTER TABLE rootcx_system.app_integrations ADD CONSTRAINT fk_app_integrations_app
                    FOREIGN KEY (app_id) REFERENCES rootcx_system.apps (id) ON DELETE CASCADE;
            END IF;
            IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'fk_app_integrations_connection') THEN
                DELETE FROM rootcx_system.app_integrations b
                    WHERE b.connection_id IS NOT NULL AND NOT EXISTS
                        (SELECT 1 FROM rootcx_system.integration_connections c WHERE c.id = b.connection_id);
                ALTER TABLE rootcx_system.app_integrations ADD CONSTRAINT fk_app_integrations_connection
                    FOREIGN KEY (connection_id) REFERENCES rootcx_system.integration_connections (id) ON DELETE CASCADE;
            END IF;
        END $$",
    ] {
        sqlx::query(ddl).execute(pool).await.map_err(crate::RuntimeError::Schema)?;
    }

    // Migrate old __delegate__ label to kind column
    sqlx::query(
        "UPDATE rootcx_system.integration_connections SET kind = 'delegation', label = NULL
         WHERE label = '__delegate__' AND kind = 'direct'"
    ).execute(pool).await.map_err(crate::RuntimeError::Schema)?;

    migrate_legacy_iuc_keys(pool).await?;

    Ok(())
}

async fn migrate_legacy_iuc_keys(pool: &sqlx::PgPool) -> Result<(), crate::RuntimeError> {
    let has_legacy: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM rootcx_system.secrets WHERE key_name LIKE '_iuc.%' LIMIT 1)"
    ).fetch_one(pool).await.unwrap_or(false);
    if !has_legacy { return Ok(()); }

    sqlx::query(
        "INSERT INTO rootcx_system.integration_connections (id, integration_id, user_id, label, kind)
         SELECT 'legacy-' || s.app_id || '-' || split_part(s.key_name, '.', 3),
                s.app_id,
                split_part(s.key_name, '.', 3),
                a.name,
                'direct'
         FROM rootcx_system.secrets s
         JOIN rootcx_system.apps a ON a.id = s.app_id
         WHERE s.key_name LIKE '_iuc.%'
           AND split_part(s.key_name, '.', 3) ~ '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
         ON CONFLICT (id) DO UPDATE SET label = EXCLUDED.label"
    ).execute(pool).await.map_err(crate::RuntimeError::Schema)?;

    // Raw copy at encrypted level — no decryption needed
    sqlx::query(
        "INSERT INTO rootcx_system.secrets (app_id, key_name, nonce, ciphertext)
         SELECT s.app_id,
                '_conn.' || 'legacy-' || s.app_id || '-' || split_part(s.key_name, '.', 3),
                s.nonce,
                s.ciphertext
         FROM rootcx_system.secrets s
         WHERE s.key_name LIKE '_iuc.%'
           AND split_part(s.key_name, '.', 3) ~ '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'
         ON CONFLICT (app_id, key_name) DO NOTHING"
    ).execute(pool).await.map_err(crate::RuntimeError::Schema)?;

    sqlx::query(
        "DELETE FROM rootcx_system.secrets
         WHERE key_name LIKE '_iuc.%'
           AND split_part(key_name, '.', 3) ~ '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$'"
    ).execute(pool).await.map_err(crate::RuntimeError::Schema)?;

    // Enrich labels from sync_cursors (gmail stores the email as `handle`)
    sqlx::query(
        "UPDATE rootcx_system.integration_connections ic
         SET label = sc.handle
         FROM gmail.sync_cursors sc
         WHERE ic.user_id = sc.user_id
           AND ic.integration_id = 'gmail'
           AND sc.handle IS NOT NULL
           AND (ic.label IS NULL OR ic.label = 'Gmail')"
    ).execute(pool).await.ok();

    Ok(())
}

pub(crate) async fn create_connection(
    pool: &sqlx::PgPool,
    integration_id: &str,
    user_id: &str,
    label: Option<&str>,
    kind: &str,
    config_id: Option<&str>,
) -> Result<String, ApiError> {
    let id = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO rootcx_system.integration_connections (id, integration_id, user_id, label, kind, config_id)
         VALUES ($1, $2, $3, $4, $5, $6)"
    )
    .bind(&id)
    .bind(integration_id)
    .bind(user_id)
    .bind(label)
    .bind(kind)
    .bind(config_id)
    .execute(pool).await?;
    Ok(id)
}

/// Publish the account metadata and encrypted credential together, after any refresh finishes.
pub(crate) async fn upsert_connection(
    pool: &sqlx::PgPool,
    secrets: &crate::secrets::SecretManager,
    integration_id: &str,
    user_id: &str,
    label: Option<&str>,
    config_id: Option<&str>,
    credentials: &JsonValue,
) -> Result<String, ApiError> {
    let (nonce, ciphertext) = secrets.encrypt(&credentials.to_string())?;
    let mut tx = pool.begin().await?;
    sqlx::query("SET LOCAL lock_timeout = '20s'").execute(&mut *tx).await?;
    let existing: Option<String> = if let Some(label) = label {
        // Serialize callbacks for the same account before deciding whether it exists.
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 73432))")
            .bind(serde_json::to_string(&(integration_id, user_id, label)).unwrap())
            .execute(&mut *tx).await?;
        sqlx::query_scalar("SELECT id FROM rootcx_system.integration_connections
            WHERE integration_id = $1 AND user_id = $2 AND label = $3 AND kind = 'direct'")
            .bind(integration_id).bind(user_id).bind(label).fetch_optional(&mut *tx).await?
    } else { None };
    let id = existing.clone().unwrap_or_else(|| Uuid::new_v4().to_string());
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 73431))")
        .bind(format!("{integration_id}:{id}")).execute(&mut *tx).await?;
    if existing.is_some() {
        let updated = sqlx::query("UPDATE rootcx_system.integration_connections
            SET status = 'active', last_error = NULL, last_error_at = NULL, config_id = $2 WHERE id = $1")
            .bind(&id).bind(config_id).execute(&mut *tx).await?;
        if updated.rows_affected() == 0 { return Err(ApiError::Conflict("Connection was removed; authorize again".into())); }
    } else {
        sqlx::query("INSERT INTO rootcx_system.integration_connections
            (id, integration_id, user_id, label, kind, config_id) VALUES ($1,$2,$3,$4,'direct',$5)")
            .bind(&id).bind(integration_id).bind(user_id).bind(label).bind(config_id).execute(&mut *tx).await?;
    }
    sqlx::query("INSERT INTO rootcx_system.secrets (app_id, key_name, nonce, ciphertext) VALUES ($1,$2,$3,$4)
        ON CONFLICT (app_id, key_name) DO UPDATE SET nonce=EXCLUDED.nonce, ciphertext=EXCLUDED.ciphertext")
        .bind(integration_id).bind(credential_key(&id)).bind(nonce).bind(ciphertext).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(id)
}

/// After an integration RPC, flag the connection dead if the result reports a
/// credential/auth failure (the worker's `INSUFFICIENT_PERMISSIONS`). Every send
/// path funnels through here instead of re-checking the result inline.
///
/// Only the active→dead transition is acted on (`status <> 'dead'`), so retries
/// against an already-dead connection neither re-stamp `last_error_at` nor spam
/// the log — the WARN fires exactly once, when a live mailbox first dies.
/// Best-effort: flagging must never mask the underlying call's own error.
pub(crate) async fn flag_if_auth_failed(
    pool: &sqlx::PgPool, secrets: &crate::secrets::SecretManager, integration_id: &str,
    connection_id: Option<&str>, version: Option<&super::oauth::CredentialVersion>, result: &JsonValue,
) {
    let (Some(cid), Some(version), Some(msg)) = (connection_id, version, auth_failure_message(result)) else { return };
    let Ok(mut tx) = pool.begin().await else { return };
    if sqlx::query("SET LOCAL lock_timeout = '20s'").execute(&mut *tx).await.is_err() { return; }
    if sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 73431))")
        .bind(format!("{integration_id}:{cid}")).execute(&mut *tx).await.is_err() { return; }
    let key = credential_key(cid);
    let stored: Option<(Vec<u8>, Vec<u8>)> = sqlx::query_as(
        "SELECT nonce, ciphertext FROM rootcx_system.secrets WHERE app_id = $1 AND key_name = $2"
    ).bind(integration_id).bind(&key).fetch_optional(&mut *tx).await.ok().flatten();
    let Some((nonce, ciphertext)) = stored else { return };
    let Some(blob) = secrets.decrypt(&nonce, &ciphertext).ok()
        .and_then(|raw| serde_json::from_str::<JsonValue>(&raw).ok()) else { return };
    if &super::oauth::CredentialVersion::of(&blob) != version { return; }

    let transitioned: Option<(Option<String>,)> = sqlx::query_as(
        "UPDATE rootcx_system.integration_connections
         SET status = 'dead', last_error = $2, last_error_at = NOW()
         WHERE id = $1 AND status <> 'dead' AND EXISTS (SELECT 1 FROM rootcx_system.secrets
             WHERE app_id = $3 AND key_name = $4 AND nonce = $5 AND ciphertext = $6)
         RETURNING label"
    )
    .bind(cid)
    .bind(&msg).bind(integration_id).bind(&key).bind(nonce).bind(ciphertext)
    .fetch_optional(&mut *tx).await.ok().flatten();

    if tx.commit().await.is_err() { return; }
    if let Some((label,)) = transitioned {
        tracing::warn!(
            integration_id,
            connection_id = cid,
            label = label.as_deref().unwrap_or(""),
            error = %msg,
            "integration connection credentials rejected — flagged dead, reconnect required"
        );
    }
}

/// Error codes that mean "the provider rejected these credentials".
///
/// A closed set owned here, never read from a manifest: this list is what an
/// untrusted worker can say to kill its own connection, so it grows only
/// deliberately.
///
/// `forbidden` is absent because a 403 is usually a missing scope on one endpoint
/// rather than a dead credential. That is not a guarantee the Core can make alone:
/// the bundled Google integrations already map their own 403s onto
/// INSUFFICIENT_PERMISSIONS (`google_calendar/lib/errors.ts:78`), so for those the
/// decision sits in the integration and not here.
const AUTH_FAILURE_CODES: [&str; 2] = ["INSUFFICIENT_PERMISSIONS", "unauthorized"];

/// A worker's credential-rejection envelope; return its message, else None.
pub(super) fn auth_failure_message(result: &JsonValue) -> Option<String> {
    if result.get("ok").and_then(|v| v.as_bool()) != Some(false) {
        return None;
    }
    let err = result.get("error")?;
    let code = err.get("code").and_then(|v| v.as_str())?;
    AUTH_FAILURE_CODES.contains(&code).then(|| {
        err.get("message").and_then(|v| v.as_str()).unwrap_or_default().to_string()
    })
}

pub(crate) fn credential_key(connection_id: &str) -> String {
    format!("{}{connection_id}", crate::secrets::CONN_KEY_PREFIX)
}

/// Find user's first live direct (non-delegation) connection for an integration.
/// Excludes dead connections: this picks a mailbox on the user's behalf, so a
/// rejected one must never be auto-selected.
pub(super) async fn first_direct_connection(
    pool: &sqlx::PgPool, integration_id: &str, user_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT id FROM rootcx_system.integration_connections
         WHERE integration_id = $1 AND user_id = $2 AND kind = 'direct' AND status = 'active'
         ORDER BY created_at LIMIT 1"
    )
    .bind(integration_id)
    .bind(user_id)
    .fetch_optional(pool).await?;
    Ok(row.map(|(id,)| id))
}

/// The binding-as-consent rule, owned by this module alongside the resolution
/// rules: acting as oneself is allowed by one's own binding OR the app-wide
/// one; acting as another user requires THAT user's own consent. A user's own
/// consent is their (app × user) binding or their default connection, which
/// they chose for every app.
pub(crate) async fn binding_allows(
    pool: &sqlx::PgPool, app_id: &str, integration_id: &str,
    requester: uuid::Uuid, effective: uuid::Uuid,
) -> Result<bool, String> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM rootcx_system.app_integrations
             WHERE app_id = $1 AND integration_id = $2 AND enabled = true
               AND (user_id = $3 OR ($4 AND user_id = $5)))
         OR EXISTS (SELECT 1 FROM rootcx_system.integration_connections
             WHERE integration_id = $2 AND user_id = $3 AND is_default)")
        .bind(app_id).bind(integration_id)
        .bind(effective.to_string())
        .bind(effective == requester)
        .bind(APP_WIDE)
        .fetch_one(pool).await.map_err(|e| e.to_string())
}

/// The permission gating an integration's elevated operations — creating
/// app-wide (shared) bindings and sending as another user. One builder so the
/// catalog (which registers it) and the gates (which check it) can't drift.
pub(crate) fn manage_perm(integration_id: &str) -> String {
    format!("integration:{integration_id}:manage")
}

/// Resolve which provider config + credentials to use for an integration action.
/// Unified entry point: handles app binding lookup, the user's default
/// connection, delegation, and direct fallback.
///
/// Returns `(config, credentials, effective_user_id, connection_id, credential_version)`. `config` is
/// the OAuth client of the selected connection (pinned via its `config_id`), or
/// the default `_platform` config when no stored connection is selected.
/// `connection_id` is `Some` whenever a stored connection was selected, so
/// callers can flag it dead only if the provider rejects that same credential version.
pub(crate) async fn resolve_credentials(
    secrets: &crate::secrets::SecretManager, pool: &sqlx::PgPool,
    integration_id: &str, user_id: &str, app_id: Option<&str>,
) -> Result<Resolved, super::oauth::RenewError> {
    // 1. Explicit app binding — the user's own (app × user) row wins over the
    //    app-wide ('') default. This is what lets the same user route app A
    //    through one mailbox and app B through another.
    if let Some(aid) = app_id {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT connection_id FROM rootcx_system.app_integrations
             WHERE app_id = $1 AND integration_id = $2 AND enabled = true
               AND user_id IN ($4, $3) AND connection_id IS NOT NULL
             ORDER BY (user_id = $3) DESC LIMIT 1"
        )
        .bind(aid)
        .bind(integration_id)
        .bind(user_id)
        .bind(APP_WIDE)
        .fetch_optional(pool).await.ok().flatten();

        if let Some((conn_id,)) = row {
            return resolve_by_connection_id(secrets, pool, integration_id, &conn_id, user_id).await;
        }
    }

    // 2. The user's default connection — their own choice for every app that
    //    has none above.
    let default: Option<(String,)> = sqlx::query_as(
        "SELECT id FROM rootcx_system.integration_connections
         WHERE integration_id = $1 AND user_id = $2 AND is_default"
    )
    .bind(integration_id)
    .bind(user_id)
    .fetch_optional(pool).await.ok().flatten();

    if let Some((conn_id,)) = default {
        return resolve_by_connection_id(secrets, pool, integration_id, &conn_id, user_id).await;
    }

    // 3. Fallback selection for the user. Only 'active' connections are
    //    eligible: a dead mailbox must never be auto-picked here. (An explicit
    //    binding or default above, or a delegation source below, is resolved by
    //    id as-is — we honor the deliberate choice rather than silently reroute.)
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT id, kind FROM rootcx_system.integration_connections
         WHERE integration_id = $1 AND user_id = $2 AND status = 'active'
         ORDER BY (kind = 'delegation') DESC, created_at
         LIMIT 2"
    )
    .bind(integration_id)
    .bind(user_id)
    .fetch_all(pool).await.unwrap_or_default();

    let delegation_row = rows.iter().find(|(_, k)| k == "delegation");
    let direct_row = rows.iter().find(|(_, k)| k == "direct");

    // 4. Try delegation
    if let Some((delegate_conn_id, _)) = delegation_row {
        let conn_key = credential_key(delegate_conn_id);
        if let Ok(Some(raw)) = secrets.get(pool, integration_id, &conn_key).await {
            let val: JsonValue = serde_json::from_str(&raw).unwrap_or(JsonValue::Null);
            if let Some(source_conn_id) = val.get("_delegate").and_then(|v| v.as_str()) {
                return resolve_by_connection_id(secrets, pool, integration_id, source_conn_id, user_id).await;
            }
        }
    }

    // 5. Direct connection
    if let Some((conn_id, _)) = direct_row {
        return resolve_by_connection_id(secrets, pool, integration_id, conn_id, user_id).await;
    }

    let config = super::routes::resolve_config_scoped(pool, secrets, integration_id, None)
        .await.unwrap_or(JsonValue::Null);
    Ok((super::managed::worker_config(integration_id, config), JsonValue::Null, user_id.to_string(), None, None))
}

/// `(config, credentials, effective_user_id, connection_id)`.
pub(crate) type Resolved = (JsonValue, JsonValue, String, Option<String>, Option<super::oauth::CredentialVersion>);

/// Resolve a connection's config and credentials, renewing the access token first
/// when the Core manages this connection's OAuth.
///
/// Fallible only because of renewal: a renewal that reached the provider and then
/// failed to persist must fail the call, since the refresh token it consumed is
/// gone and serving the access token anyway would hide that until the next call.
pub(crate) async fn resolve_by_connection_id(
    secrets: &crate::secrets::SecretManager, pool: &sqlx::PgPool,
    integration_id: &str, connection_id: &str, fallback_user: &str,
) -> Result<Resolved, super::oauth::RenewError> {
    let conn_key = credential_key(connection_id);
    // Read the pinned client and encrypted grant from one committed snapshot.
    let row: Option<(String, Option<String>, Option<Vec<u8>>, Option<Vec<u8>>)> = sqlx::query_as(
        "SELECT c.user_id, c.config_id, s.nonce, s.ciphertext
         FROM rootcx_system.integration_connections c LEFT JOIN rootcx_system.secrets s
           ON s.app_id = c.integration_id AND s.key_name = $3
         WHERE c.id = $1 AND c.integration_id = $2"
    ).bind(connection_id).bind(integration_id).bind(&conn_key).fetch_optional(pool).await
        .map_err(|_| super::oauth::RenewError::Transient("Connection snapshot unavailable".into()))?;
    let (effective_user, config_id, nonce, ciphertext) = row
        .unwrap_or_else(|| (fallback_user.to_string(), None, None, None));
    let config = super::routes::resolve_config_scoped(pool, secrets, integration_id, config_id.as_deref())
        .await.map_err(|_| super::oauth::RenewError::Transient("OAuth configuration unavailable".into()))?;
    if let (Some(nonce), Some(ciphertext)) = (nonce, ciphertext) {
        let raw = secrets.decrypt(&nonce, &ciphertext)
            .map_err(|_| super::oauth::RenewError::Transient("Connection credential unavailable".into()))?;
        let creds: JsonValue = serde_json::from_str(&raw)
            .map_err(|_| super::oauth::RenewError::Transient("Connection credential invalid".into()))?;
        require_grant_owner(&creds, &effective_user, fallback_user)?;
        let (creds, version) = super::oauth::credentials_for_worker_versioned(
            pool, secrets, integration_id, connection_id, &config, creds, config_id.as_deref(),
        ).await?;
        Ok((super::managed::worker_config(integration_id, config), creds, effective_user, Some(connection_id.to_string()), Some(version)))
    } else {
        Ok((super::managed::worker_config(integration_id, config), JsonValue::Null, fallback_user.to_string(), None, None))
    }
}

fn require_grant_owner(credentials: &JsonValue, owner: &str, requester: &str) -> Result<(), super::oauth::RenewError> {
    if credentials["hubspotUserLevel"] == true && owner != requester {
        return Err(super::oauth::RenewError::Rejected("Connect your own HubSpot account; user-level grants cannot be shared".into()));
    }
    Ok(())
}

pub(super) async fn verify_owner(
    pool: &sqlx::PgPool,
    connection_id: &str,
    integration_id: &str,
    identity: &Identity,
) -> Result<(), ApiError> {
    let owner: Option<(String,)> = sqlx::query_as(
        "SELECT user_id FROM rootcx_system.integration_connections
         WHERE id = $1 AND integration_id = $2"
    )
    .bind(connection_id)
    .bind(integration_id)
    .fetch_optional(pool).await?;

    match owner {
        Some((uid,)) if uid == identity.user_id.to_string() => Ok(()),
        Some(_) => Err(ApiError::Forbidden("not your connection".into())),
        None => Err(ApiError::NotFound("connection not found".into())),
    }
}

pub async fn list_connections(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(integration_id): Path<String>,
) -> Result<Json<Vec<Connection>>, ApiError> {
    let pool = routes::pool(&rt);
    let rows: Vec<(String, String, String, Option<String>, chrono::DateTime<chrono::Utc>, String, bool)> = sqlx::query_as(
        "SELECT id, integration_id, user_id, label, created_at, status, is_default
         FROM rootcx_system.integration_connections
         WHERE integration_id = $1 AND user_id = $2 AND kind = 'direct'
         ORDER BY created_at"
    )
    .bind(&integration_id)
    .bind(identity.user_id.to_string())
    .fetch_all(&pool).await?;

    let connections: Vec<Connection> = rows.into_iter().map(|(id, iid, uid, label, created_at, status, is_default)| {
        Connection { id, integration_id: iid, user_id: uid, label, created_at: created_at.to_rfc3339(), status, is_default }
    }).collect();

    Ok(Json(connections))
}

pub async fn delete_connection(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((integration_id, connection_id)): Path<(String, String)>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt);
    verify_owner(&pool, &connection_id, &integration_id, &identity).await?;

    // A failed secret deletion must not leave a usable orphan credential behind.
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM rootcx_system.secrets WHERE app_id = $1 AND key_name = $2")
        .bind(&integration_id).bind(credential_key(&connection_id)).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM rootcx_system.integration_connections WHERE id = $1")
        .bind(&connection_id).execute(&mut *tx).await?;
    tx.commit().await?;

    Ok(Json(json!({ "message": "connection deleted" })))
}

/// Make one of your own accounts the default for every app without an account
/// choice of its own; the previous default, if any, stops being one.
pub async fn set_default_connection(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((integration_id, connection_id)): Path<(String, String)>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt);
    verify_owner(&pool, &connection_id, &integration_id, &identity).await?;

    // Locking the owner's accounts serializes concurrent switches, so the one
    // default per (integration × user) is never contended mid-transaction. Only
    // direct accounts qualify: a delegation's secret is a pointer, not a credential.
    let owner = identity.user_id.to_string();
    let mut tx = pool.begin().await?;
    let accounts: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM rootcx_system.integration_connections
         WHERE integration_id = $1 AND user_id = $2 AND kind = 'direct' FOR UPDATE"
    ).bind(&integration_id).bind(&owner).fetch_all(&mut *tx).await?;
    if !accounts.contains(&connection_id) {
        return Err(ApiError::NotFound("connection not found".into()));
    }
    sqlx::query(
        "UPDATE rootcx_system.integration_connections SET is_default = false
         WHERE integration_id = $1 AND user_id = $2 AND is_default AND id <> $3"
    ).bind(&integration_id).bind(&owner).bind(&connection_id).execute(&mut *tx).await?;
    sqlx::query("UPDATE rootcx_system.integration_connections SET is_default = true WHERE id = $1")
        .bind(&connection_id).execute(&mut *tx).await?;
    tx.commit().await?;

    Ok(Json(json!({ "message": "default connection set" })))
}

pub async fn clear_default_connection(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((integration_id, connection_id)): Path<(String, String)>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt);
    verify_owner(&pool, &connection_id, &integration_id, &identity).await?;
    sqlx::query("UPDATE rootcx_system.integration_connections SET is_default = false WHERE id = $1")
        .bind(&connection_id).execute(&pool).await?;

    Ok(Json(json!({ "message": "default connection cleared" })))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateConnectionBody {
    pub label: Option<String>,
}

pub async fn update_connection(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((integration_id, connection_id)): Path<(String, String)>,
    Json(body): Json<UpdateConnectionBody>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt);
    verify_owner(&pool, &connection_id, &integration_id, &identity).await?;

    if let Some(ref label) = body.label {
        sqlx::query("UPDATE rootcx_system.integration_connections SET label = $1 WHERE id = $2")
            .bind(label)
            .bind(&connection_id)
            .execute(&pool).await?;
    }

    Ok(Json(json!({ "message": "updated" })))
}

pub async fn list_app_bindings(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
) -> Result<Json<Vec<JsonValue>>, ApiError> {
    let pool = routes::pool(&rt);
    let rows: Vec<(String, bool, Option<String>, String, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT integration_id, enabled, connection_id, user_id, created_at
         FROM rootcx_system.app_integrations WHERE app_id = $1 AND user_id IN ('', $2)"
    )
    .bind(&app_id).bind(identity.user_id.to_string())
    .fetch_all(&pool).await?;

    let bindings: Vec<JsonValue> = rows.into_iter().map(|(iid, enabled, conn_id, user_id, created_at)| {
        json!({
            "integrationId": iid,
            "enabled": enabled,
            "connectionId": conn_id,
            "userId": (user_id != APP_WIDE).then_some(user_id),
            "createdAt": created_at.to_rfc3339(),
        })
    }).collect();

    Ok(Json(bindings))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BindBody {
    pub integration_id: String,
    pub connection_id: Option<String>,
    /// "user" → binding scoped to the calling user (their consent for this app
    /// to use the given connection, incl. from background jobs). Default: app-wide.
    pub scope: Option<String>,
}

/// '' (app-wide) unless scope=user, then the caller's own id.
fn scope_user_id(scope: Option<&str>, identity: &Identity) -> String {
    if scope == Some(SCOPE_USER) { identity.user_id.to_string() } else { APP_WIDE.to_string() }
}

pub async fn bind_app(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
    Json(body): Json<BindBody>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt);
    let binding_user = scope_user_id(body.scope.as_deref(), &identity);

    if binding_user != APP_WIDE && body.connection_id.is_none() {
        return Err(ApiError::BadRequest("a user-scoped binding requires connectionId".into()));
    }

    if let Some(ref conn_id) = body.connection_id {
        let exists: Option<(String,)> = sqlx::query_as(
            "SELECT id FROM rootcx_system.integration_connections
             WHERE id = $1 AND integration_id = $2 AND user_id = $3"
        )
        .bind(conn_id)
        .bind(&body.integration_id)
        .bind(identity.user_id.to_string())
        .fetch_optional(&pool).await?;
        if exists.is_none() {
            return Err(ApiError::BadRequest("connection not found or not owned by you".into()));
        }
    }

    // App-wide bindings expose a connection to every holder of the integration's
    // send permission, so creating one requires the elevated manage permission.
    // User-scoped bindings (your own connection, your own consent) stay open.
    if binding_user == APP_WIDE {
        crate::governance::authority::require_perm(
            &pool, identity.user_id, &manage_perm(&body.integration_id),
        ).await?;
    }

    sqlx::query(
        "INSERT INTO rootcx_system.app_integrations (app_id, integration_id, connection_id, user_id)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (app_id, integration_id, user_id)
         DO UPDATE SET connection_id = EXCLUDED.connection_id, enabled = true"
    )
    .bind(&app_id)
    .bind(&body.integration_id)
    .bind(&body.connection_id)
    .bind(&binding_user)
    .execute(&pool).await?;

    Ok(Json(json!({ "message": "bound" })))
}

#[derive(Debug, Deserialize)]
pub struct UnbindQuery {
    pub scope: Option<String>,
}

pub async fn unbind_app(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((app_id, integration_id)): Path<(String, String)>,
    axum::extract::Query(q): axum::extract::Query<UnbindQuery>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt);
    // ?scope=user removes only the caller's own binding; default removes the app-wide one.
    let binding_user = scope_user_id(q.scope.as_deref(), &identity);
    sqlx::query(
        "DELETE FROM rootcx_system.app_integrations
         WHERE app_id = $1 AND integration_id = $2 AND user_id = $3"
    )
    .bind(&app_id)
    .bind(&integration_id)
    .bind(&binding_user)
    .execute(&pool).await?;

    Ok(Json(json!({ "message": "unbound" })))
}

pub async fn connected_users(
    pool: &sqlx::PgPool, integration_id: &str,
) -> Result<Vec<String>, ApiError> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT DISTINCT user_id FROM rootcx_system.integration_connections
         WHERE integration_id = $1 AND kind = 'direct'"
    )
    .bind(integration_id)
    .fetch_all(pool).await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// All active direct connections for an integration: (user_id, connection_id).
/// Used by `syncConnectedUsers` to fan-out per-connection (not per-user), so
/// each sync resolves credentials for exactly its own mailbox.
pub async fn connected_connections(
    pool: &sqlx::PgPool, integration_id: &str,
) -> Result<Vec<(String, String)>, ApiError> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT user_id, id FROM rootcx_system.integration_connections
         WHERE integration_id = $1 AND kind = 'direct' AND status = 'active'
         ORDER BY user_id, created_at"
    )
    .bind(integration_id)
    .fetch_all(pool).await?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_level_grants_cannot_be_borrowed_through_shared_bindings_or_delegation() {
        for personal in [false, true] {
            let credentials = json!({"hubspotUserLevel": personal});
            assert!(require_grant_owner(&credentials, "owner", "owner").is_ok());
            assert_eq!(require_grant_owner(&credentials, "owner", "another-user").is_err(), personal);
        }
    }

    #[test]
    fn auth_failure_message_classifies_worker_envelope() {
        // (case, worker result envelope, expected dead-flagging message)
        let cases: &[(&str, JsonValue, Option<&str>)] = &[
            ("success is never a failure",
                json!({ "ok": true, "data": {} }), None),
            ("missing ok field is not a failure",
                json!({ "data": {} }), None),
            ("a non-auth error is not a credential failure",
                json!({ "ok": false, "error": { "code": "TEMPORARY_ERROR", "message": "rate limited" } }), None),
            ("a provider's own rejection code flags dead, not just Gmail's spelling",
                json!({ "ok": false, "error": { "code": "unauthorized", "message": "invalid_grant" } }), Some("invalid_grant")),
            ("forbidden is a missing scope on one endpoint, never a dead credential",
                json!({ "ok": false, "error": { "code": "forbidden", "message": "insufficient scope" } }), None),
            ("failure without an error object is not classified",
                json!({ "ok": false }), None),
            ("INSUFFICIENT_PERMISSIONS flags dead and carries its message",
                json!({ "ok": false, "error": { "code": "INSUFFICIENT_PERMISSIONS", "message": "invalid_grant" } }), Some("invalid_grant")),
            ("INSUFFICIENT_PERMISSIONS with no message still flags dead",
                json!({ "ok": false, "error": { "code": "INSUFFICIENT_PERMISSIONS" } }), Some("")),
        ];
        for (case, envelope, expected) in cases {
            assert_eq!(auth_failure_message(envelope).as_deref(), *expected, "case: {case}");
        }
    }

    #[test]
    fn credential_key_format_is_stable() {
        assert_eq!(credential_key("abc-123"), "_conn.abc-123");
        assert_eq!(credential_key("legacy-gmail-user1"), "_conn.legacy-gmail-user1");
    }

    mod against_the_database {
        use super::*;
        use crate::extensions::test_db::pool;
        use crate::secrets::SecretManager;

        /// One integration and its apps, under ids no other test shares.
        struct Tenant {
            pool: sqlx::PgPool,
            secrets: SecretManager,
            integration: String,
            tag: String,
        }

        impl Tenant {
            async fn new() -> Self {
                let tag = Uuid::new_v4().simple().to_string()[..12].to_string();
                Self { pool: pool().await, secrets: SecretManager::with_key(&[0x3C; 32]), integration: format!("int_{tag}"), tag }
            }

            async fn install(&self, name: &str) -> String {
                let id = format!("app_{}_{name}", self.tag);
                sqlx::query("INSERT INTO rootcx_system.apps (id, name) VALUES ($1, $1)")
                    .bind(&id).execute(&self.pool).await.unwrap();
                id
            }

            /// A live account whose stored credential names it, so a resolution shows which one it reached.
            async fn connect(&self, user: Uuid, account: &str) -> String {
                let id = create_connection(&self.pool, &self.integration, &user.to_string(), Some(account), "direct", None).await.unwrap();
                self.secrets.set(&self.pool, &self.integration, &credential_key(&id), &json!({ "account": account }).to_string())
                    .await.unwrap();
                id
            }

            async fn make_default(&self, connection: &str) {
                sqlx::query("UPDATE rootcx_system.integration_connections SET is_default = true WHERE id = $1")
                    .bind(connection).execute(&self.pool).await.unwrap();
            }

            async fn bind(&self, app: &str, user: &str, connection: &str) {
                sqlx::query("INSERT INTO rootcx_system.app_integrations (app_id, integration_id, user_id, connection_id) VALUES ($1, $2, $3, $4)")
                    .bind(app).bind(&self.integration).bind(user).bind(connection).execute(&self.pool).await.unwrap();
            }

            async fn account(&self, user: Uuid, app: &str) -> Option<String> {
                let (_, credentials, ..) = resolve_credentials(&self.secrets, &self.pool, &self.integration, &user.to_string(), Some(app))
                    .await.unwrap();
                credentials["account"].as_str().map(str::to_string)
            }
        }

        #[tokio::test]
        async fn each_person_reaches_their_own_account_from_every_app() {
            let t = Tenant::new().await;
            let (plain, chosen, shared) = (t.install("plain").await, t.install("chosen").await, t.install("shared").await);
            let [alice, bob, carol, dave, admin, agent] = [(); 6].map(|_| Uuid::new_v4());

            let alice_main = t.connect(alice, "alice-main").await;
            t.make_default(&alice_main).await;
            // Alice uses a second account for one app.
            let alice_other = t.connect(alice, "alice-other").await;
            t.bind(&chosen, &alice.to_string(), &alice_other).await;
            let bob_main = t.connect(bob, "bob-main").await;
            t.make_default(&bob_main).await;
            // One app runs on a shared mailbox chosen for everyone.
            let mailbox = t.connect(admin, "shared-mailbox").await;
            t.bind(&shared, APP_WIDE, &mailbox).await;
            // Dave's default was rejected by the provider while another account still works.
            let dave_main = t.connect(dave, "dave-main").await;
            t.make_default(&dave_main).await;
            t.connect(dave, "dave-other").await;
            sqlx::query("UPDATE rootcx_system.integration_connections SET status = 'dead' WHERE id = $1")
                .bind(&dave_main).execute(&t.pool).await.unwrap();

            // (case, app, requester, acting for, account reached; None = refused)
            let cases = [
                ("a default serves an app without a choice of its own", &plain, alice, alice, Some("alice-main")),
                ("an app's own account choice beats the default", &chosen, alice, alice, Some("alice-other")),
                ("an app's shared account beats a personal default", &shared, alice, alice, Some("shared-mailbox")),
                ("a colleague reaches their own default", &plain, bob, bob, Some("bob-main")),
                ("another person's choice for an app never applies to a colleague", &chosen, bob, bob, Some("bob-main")),
                ("an agent acting for a person reaches that person's default", &plain, agent, alice, Some("alice-main")),
                ("the same agent acting for a colleague reaches the colleague's", &plain, agent, bob, Some("bob-main")),
                ("a dead default is reported, never silently swapped", &plain, dave, dave, Some("dave-main")),
                ("someone who never connected is refused", &plain, carol, carol, None),
                ("an agent cannot act for someone who never connected", &plain, agent, carol, None),
            ];
            for (case, app, requester, effective, expected) in cases {
                let allowed = binding_allows(&t.pool, app, &t.integration, requester, effective).await.unwrap();
                assert_eq!(allowed, expected.is_some(), "consent: {case}");
                if allowed {
                    assert_eq!(t.account(effective, app).await.as_deref(), expected, "account: {case}");
                }
            }
        }
    }
}
