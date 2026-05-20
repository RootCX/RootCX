use axum::Json;
use axum::extract::{Path, State};
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use uuid::Uuid;

use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::routes::{self, SharedRuntime};

const CONN_PREFIX: &str = "_conn";

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
) -> Result<String, ApiError> {
    let id = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO rootcx_system.integration_connections (id, integration_id, user_id, label, kind)
         VALUES ($1, $2, $3, $4, $5)"
    )
    .bind(&id)
    .bind(integration_id)
    .bind(user_id)
    .bind(label)
    .bind(kind)
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
            return Ok(id);
        }
    }

    create_connection(pool, integration_id, user_id, label, "direct").await
}

pub(crate) fn credential_key(connection_id: &str) -> String {
    format!("{CONN_PREFIX}.{connection_id}")
}

/// Find user's first direct (non-delegation) connection for an integration.
pub(super) async fn first_direct_connection(
    pool: &sqlx::PgPool, integration_id: &str, user_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT id FROM rootcx_system.integration_connections
         WHERE integration_id = $1 AND user_id = $2 AND kind = 'direct'
         ORDER BY created_at LIMIT 1"
    )
    .bind(integration_id)
    .bind(user_id)
    .fetch_optional(pool).await?;
    Ok(row.map(|(id,)| id))
}

/// Resolve which credentials to use for an integration action.
/// Unified entry point: handles app binding lookup, delegation, and direct fallback.
pub(crate) async fn resolve_credentials(
    secrets: &crate::secrets::SecretManager, pool: &sqlx::PgPool,
    integration_id: &str, user_id: &str, app_id: Option<&str>,
) -> (JsonValue, String) {
    // 1. Check explicit app binding
    if let Some(aid) = app_id {
        let row: Option<(Option<String>,)> = sqlx::query_as(
            "SELECT connection_id FROM rootcx_system.app_integrations
             WHERE app_id = $1 AND integration_id = $2 AND enabled = true"
        )
        .bind(aid)
        .bind(integration_id)
        .fetch_optional(pool).await.ok().flatten();

        if let Some((Some(conn_id),)) = row {
            return resolve_by_connection_id(secrets, pool, integration_id, &conn_id, user_id).await;
        }
    }

    // 2. Single query: fetch delegation + direct connections for this user
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT id, kind FROM rootcx_system.integration_connections
         WHERE integration_id = $1 AND user_id = $2
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

    (JsonValue::Null, user_id.to_string())
}

async fn resolve_by_connection_id(
    secrets: &crate::secrets::SecretManager, pool: &sqlx::PgPool,
    integration_id: &str, connection_id: &str, fallback_user: &str,
) -> (JsonValue, String) {
    let conn_key = credential_key(connection_id);
    match secrets.get(pool, integration_id, &conn_key).await {
        Ok(Some(raw)) => {
            let creds: JsonValue = serde_json::from_str(&raw).unwrap_or(JsonValue::Null);
            let effective_user: String = sqlx::query_scalar(
                "SELECT user_id FROM rootcx_system.integration_connections WHERE id = $1"
            ).bind(connection_id).fetch_optional(pool).await.ok().flatten()
                .unwrap_or_else(|| fallback_user.to_string());
            (creds, effective_user)
        }
        _ => (JsonValue::Null, fallback_user.to_string()),
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
    let rows: Vec<(String, bool, Option<String>, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT integration_id, enabled, connection_id, created_at
         FROM rootcx_system.app_integrations WHERE app_id = $1"
    )
    .bind(&app_id)
    .fetch_all(&pool).await?;

    let bindings: Vec<JsonValue> = rows.into_iter().map(|(iid, enabled, conn_id, created_at)| {
        json!({
            "integrationId": iid,
            "enabled": enabled,
            "connectionId": conn_id,
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
}

pub async fn bind_app(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
    Json(body): Json<BindBody>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt);

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

    sqlx::query(
        "INSERT INTO rootcx_system.app_integrations (app_id, integration_id, connection_id)
         VALUES ($1, $2, $3)
         ON CONFLICT (app_id, integration_id)
         DO UPDATE SET connection_id = EXCLUDED.connection_id, enabled = true"
    )
    .bind(&app_id)
    .bind(&body.integration_id)
    .bind(&body.connection_id)
    .execute(&pool).await?;

    Ok(Json(json!({ "message": "bound" })))
}

pub async fn unbind_app(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((app_id, integration_id)): Path<(String, String)>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt);
    sqlx::query(
        "DELETE FROM rootcx_system.app_integrations WHERE app_id = $1 AND integration_id = $2"
    )
    .bind(&app_id)
    .bind(&integration_id)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_key_format_is_stable() {
        assert_eq!(credential_key("abc-123"), "_conn.abc-123");
        assert_eq!(credential_key("legacy-gmail-user1"), "_conn.legacy-gmail-user1");
    }
}
