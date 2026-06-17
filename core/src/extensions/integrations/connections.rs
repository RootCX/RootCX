use axum::Json;
use axum::extract::{Path, State};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use uuid::Uuid;

use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::routes::{self, SharedRuntime};

const CONN_PREFIX: &str = "_conn";
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

/// Create or reuse a connection. If a direct connection with the same label
/// already exists for this user+integration, return its id (credentials will be overwritten).
/// If label is None, always creates a new connection.
pub(crate) async fn upsert_connection(
    pool: &sqlx::PgPool,
    integration_id: &str,
    user_id: &str,
    label: Option<&str>,
    config_id: Option<&str>,
) -> Result<String, ApiError> {
    if let Some(lbl) = label {
        let existing: Option<(String,)> = sqlx::query_as(
            "SELECT id FROM rootcx_system.integration_connections
             WHERE integration_id = $1 AND user_id = $2 AND label = $3 AND kind = 'direct'"
        )
        .bind(integration_id)
        .bind(user_id)
        .bind(lbl)
        .fetch_optional(pool).await?;

        if let Some((id,)) = existing {
            // Reconnecting a known mailbox: revive it and repin its config (the
            // user may have re-authed via a different OAuth client). Credentials
            // are overwritten by the caller, so any prior 'dead' flag is stale.
            let _ = sqlx::query(
                "UPDATE rootcx_system.integration_connections
                 SET status = 'active', last_error = NULL, last_error_at = NULL, config_id = $2 WHERE id = $1"
            ).bind(&id).bind(config_id).execute(pool).await;
            return Ok(id);
        }
    }

    create_connection(pool, integration_id, user_id, label, "direct", config_id).await
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
    pool: &sqlx::PgPool, integration_id: &str, connection_id: Option<&str>, result: &JsonValue,
) {
    let (Some(cid), Some(msg)) = (connection_id, auth_failure_message(result)) else { return };

    let transitioned: Option<(Option<String>,)> = sqlx::query_as(
        "UPDATE rootcx_system.integration_connections
         SET status = 'dead', last_error = $2, last_error_at = NOW()
         WHERE id = $1 AND status <> 'dead'
         RETURNING label"
    )
    .bind(cid)
    .bind(&msg)
    .fetch_optional(pool).await.ok().flatten();

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

/// The integration worker's `INSUFFICIENT_PERMISSIONS` (which Gmail returns for
/// `invalid_grant`) signals dead credentials; return its message, else None.
fn auth_failure_message(result: &JsonValue) -> Option<String> {
    if result.get("ok").and_then(|v| v.as_bool()) != Some(false) {
        return None;
    }
    let err = result.get("error")?;
    if err.get("code").and_then(|v| v.as_str()) == Some("INSUFFICIENT_PERMISSIONS") {
        Some(err.get("message").and_then(|v| v.as_str()).unwrap_or_default().to_string())
    } else {
        None
    }
}

pub(crate) fn credential_key(connection_id: &str) -> String {
    format!("{CONN_PREFIX}.{connection_id}")
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
/// one; acting as another user requires THAT user's own (app × user) binding.
pub(crate) async fn binding_allows(
    pool: &sqlx::PgPool, app_id: &str, integration_id: &str,
    requester: uuid::Uuid, effective: uuid::Uuid,
) -> Result<bool, String> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM rootcx_system.app_integrations
         WHERE app_id = $1 AND integration_id = $2 AND enabled = true
           AND (user_id = $3 OR ($4 AND user_id = $5)))")
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
/// Unified entry point: handles app binding lookup, delegation, and direct fallback.
///
/// Returns `(config, credentials, effective_user_id, connection_id)`. `config` is
/// the OAuth client of the selected connection (pinned via its `config_id`), or
/// the default `_platform` config when no stored connection is selected.
/// `connection_id` is `Some` whenever a stored connection was selected, so
/// callers can flag it dead if the provider later rejects it.
pub(crate) async fn resolve_credentials(
    secrets: &crate::secrets::SecretManager, pool: &sqlx::PgPool,
    integration_id: &str, user_id: &str, app_id: Option<&str>,
) -> (JsonValue, JsonValue, String, Option<String>) {
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

    // 2. Fallback selection for the user. Only 'active' connections are
    //    eligible: a dead mailbox must never be auto-picked here. (An explicit
    //    binding above, or a delegation source below, is resolved by id as-is —
    //    we honor the deliberate choice rather than silently reroute.)
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

    // 3. Try delegation
    if let Some((delegate_conn_id, _)) = delegation_row {
        let conn_key = credential_key(delegate_conn_id);
        if let Ok(Some(raw)) = secrets.get(pool, integration_id, &conn_key).await {
            let val: JsonValue = serde_json::from_str(&raw).unwrap_or(JsonValue::Null);
            if let Some(source_conn_id) = val.get("_delegate").and_then(|v| v.as_str()) {
                return resolve_by_connection_id(secrets, pool, integration_id, source_conn_id, user_id).await;
            }
        }
    }

    // 4. Direct connection
    if let Some((conn_id, _)) = direct_row {
        return resolve_by_connection_id(secrets, pool, integration_id, conn_id, user_id).await;
    }

    let config = super::routes::resolve_config_scoped(pool, secrets, integration_id, None)
        .await.unwrap_or(JsonValue::Null);
    (config, JsonValue::Null, user_id.to_string(), None)
}

pub(crate) async fn resolve_by_connection_id(
    secrets: &crate::secrets::SecretManager, pool: &sqlx::PgPool,
    integration_id: &str, connection_id: &str, fallback_user: &str,
) -> (JsonValue, JsonValue, String, Option<String>) {
    let conn_key = credential_key(connection_id);
    // The connection's config_id pins which OAuth client refreshes its token.
    let row: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT user_id, config_id FROM rootcx_system.integration_connections WHERE id = $1"
    ).bind(connection_id).fetch_optional(pool).await.ok().flatten();
    let (effective_user, config_id) = match row {
        Some((uid, cfg)) => (uid, cfg),
        None => (fallback_user.to_string(), None),
    };
    let config = super::routes::resolve_config_scoped(pool, secrets, integration_id, config_id.as_deref())
        .await.unwrap_or(JsonValue::Null);
    match secrets.get(pool, integration_id, &conn_key).await {
        Ok(Some(raw)) => {
            let creds: JsonValue = serde_json::from_str(&raw).unwrap_or(JsonValue::Null);
            (config, creds, effective_user, Some(connection_id.to_string()))
        }
        _ => (config, JsonValue::Null, fallback_user.to_string(), None),
    }
}

async fn verify_owner(
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
    let rows: Vec<(String, String, String, Option<String>, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT id, integration_id, user_id, label, created_at
         FROM rootcx_system.integration_connections
         WHERE integration_id = $1 AND user_id = $2 AND kind = 'direct'
         ORDER BY created_at"
    )
    .bind(&integration_id)
    .bind(identity.user_id.to_string())
    .fetch_all(&pool).await?;

    let connections: Vec<Connection> = rows.into_iter().map(|(id, iid, uid, label, created_at)| {
        Connection { id, integration_id: iid, user_id: uid, label, created_at: created_at.to_rfc3339() }
    }).collect();

    Ok(Json(connections))
}

pub async fn delete_connection(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((integration_id, connection_id)): Path<(String, String)>,
) -> Result<Json<JsonValue>, ApiError> {
    let (pool, secrets) = routes::pool_and_secrets(&rt);
    verify_owner(&pool, &connection_id, &integration_id, &identity).await?;

    let _ = secrets.delete(&pool, &integration_id, &credential_key(&connection_id)).await;

    sqlx::query("DELETE FROM rootcx_system.integration_connections WHERE id = $1")
        .bind(&connection_id)
        .execute(&pool).await?;

    Ok(Json(json!({ "message": "connection deleted" })))
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
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
) -> Result<Json<Vec<JsonValue>>, ApiError> {
    let pool = routes::pool(&rt);
    let rows: Vec<(String, bool, Option<String>, String, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT integration_id, enabled, connection_id, user_id, created_at
         FROM rootcx_system.app_integrations WHERE app_id = $1"
    )
    .bind(&app_id)
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
    fn auth_failure_message_classifies_worker_envelope() {
        // (case, worker result envelope, expected dead-flagging message)
        let cases: &[(&str, JsonValue, Option<&str>)] = &[
            ("success is never a failure",
                json!({ "ok": true, "data": {} }), None),
            ("missing ok field is not a failure",
                json!({ "data": {} }), None),
            ("a non-auth error is not a credential failure",
                json!({ "ok": false, "error": { "code": "TEMPORARY_ERROR", "message": "rate limited" } }), None),
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
}
