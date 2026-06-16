use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::Html;
use hmac::{Hmac, Mac};
use serde_json::{Value as JsonValue, json};
use sha2::Sha256;
use tokio::sync::Mutex;
use tracing::{info, warn};
use crate::governance::authority::require_admin;
use uuid::Uuid;

use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::routes::SharedRuntime;

type HmacSha256 = Hmac<Sha256>;

struct Pending { integration_id: String, user_id: String, config_id: Option<String>, callback_url: String, created: Instant }

static PENDING: LazyLock<Mutex<HashMap<String, Pending>>> = LazyLock::new(Default::default);
const TTL_SECS: u64 = 600;

fn resolve_callback_url(headers: &HeaderMap) -> String {
    std::env::var("OAUTH_CALLBACK_URL").unwrap_or_else(|_| {
        let host = headers.get("host").and_then(|v| v.to_str().ok()).unwrap_or("localhost:9100");
        let host = if host.starts_with("127.0.0.1") { host.replacen("127.0.0.1", "localhost", 1) } else { host.to_string() };
        let scheme = headers.get("x-forwarded-proto").and_then(|v| v.to_str().ok()).unwrap_or("http");
        format!("{scheme}://{host}/api/v1/integrations/auth/callback")
    })
}

fn build_callback_and_state(headers: &HeaderMap, nonce: &str) -> (String, String) {
    let cb = resolve_callback_url(headers);
    match (std::env::var("OAUTH_CALLBACK_HMAC_KEY"), std::env::var("ROOTCX_TENANT_REF")) {
        (Ok(key), Ok(tref)) => {
            let ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
            let payload = format!("{tref}:{nonce}:{ts}");
            let mut mac = HmacSha256::new_from_slice(key.as_bytes()).expect("HMAC key");
            mac.update(payload.as_bytes());
            (cb, format!("{payload}:{}", hex::encode(mac.finalize().into_bytes())))
        }
        _ => (cb, nonce.to_string()),
    }
}

fn extract_nonce(state: &str) -> &str {
    match state.split(':').collect::<Vec<_>>().as_slice() {
        [_, nonce, _, _] => nonce,
        _ => state,
    }
}

fn resolve_target_user(identity: &Identity, qs: &HashMap<String, String>) -> String {
    qs.get("agent_app_id")
        .map(|id| crate::extensions::agents::agent_user_id(id).to_string())
        .unwrap_or_else(|| identity.user_id.to_string())
}

pub async fn start(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(integration_id): Path<String>,
    axum::extract::Query(qs): axum::extract::Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Result<Json<JsonValue>, ApiError> {
    let (pool, secrets) = crate::routes::pool_and_secrets(&rt);
    let wm = crate::routes::wm(&rt);
    // ?config_id=<id> selects a named provider config (a specific OAuth client);
    // absent → the integration's default config.
    let config_id = qs.get("config_id").cloned();
    if let Some(ref cid) = config_id {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM rootcx_system.integration_configs WHERE id = $1 AND integration_id = $2)"
        ).bind(cid).bind(&integration_id).fetch_one(&pool).await?;
        if !exists {
            return Err(ApiError::NotFound("config_id not found for this integration".into()));
        }
    }
    let config = super::routes::resolve_config_scoped(&pool, &secrets, &integration_id, config_id.as_deref()).await?;

    let nonce = Uuid::new_v4().to_string();
    let (cb, state) = build_callback_and_state(&headers, &nonce);

    PENDING.lock().await.insert(nonce, Pending {
        integration_id: integration_id.clone(), user_id: identity.user_id.to_string(),
        config_id, callback_url: cb.clone(), created: Instant::now(),
    });

    let result = wm.rpc(
        &integration_id, Uuid::new_v4().to_string(), "__auth_start".into(),
        json!({ "config": config, "callbackUrl": cb, "state": state, "userId": identity.user_id }),
        None,
    ).await.map_err(|e| ApiError::Internal(e.to_string()))?;

    info!(integration_id, user_id = %identity.user_id, "auth flow started");
    Ok(Json(result))
}

pub async fn callback(
    State(rt): State<SharedRuntime>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Html<String>, ApiError> {
    let state = params.get("state").ok_or_else(|| ApiError::BadRequest("missing state".into()))?;
    let pending = {
        let mut map = PENDING.lock().await;
        map.retain(|_, p| p.created.elapsed().as_secs() < TTL_SECS);
        map.remove(extract_nonce(state))
    }.ok_or_else(|| ApiError::BadRequest("invalid or expired auth state".into()))?;

    let (pool, secrets) = crate::routes::pool_and_secrets(&rt);
    let wm = crate::routes::wm(&rt);
    let config = super::routes::resolve_config_scoped(&pool, &secrets, &pending.integration_id, pending.config_id.as_deref()).await?;

    let result = wm.rpc(
        &pending.integration_id, Uuid::new_v4().to_string(), "__auth_callback".into(),
        json!({ "config": config, "query": params, "userId": pending.user_id, "callbackUrl": pending.callback_url }),
        None,
    ).await.map_err(|e| ApiError::Internal(e.to_string()))?;

    if let Some(creds) = result.get("credentials") {
        let conn_id = super::connections::upsert_connection(
            &pool, &pending.integration_id, &pending.user_id, None, pending.config_id.as_deref(),
        ).await?;
        let conn_key = super::connections::credential_key(&conn_id);
        secrets.set(&pool, &pending.integration_id, &conn_key, &creds.to_string()).await
            .map_err(|e| ApiError::Internal(e.to_string()))?;

        // Resolve the account label (email) via get_profile now that creds are
        // persisted and the token is stable.
        let label: Option<String> = wm.rpc(
            &pending.integration_id, Uuid::new_v4().to_string(), "__integration".into(),
            json!({ "action": "get_profile", "input": {}, "config": config,
                "userCredentials": creds, "userId": pending.user_id }),
            None,
        ).await.ok()
            .and_then(|r| r.pointer("/data/emailAddress").and_then(|v| v.as_str()).map(|s| s.to_string()));
        if let Some(ref l) = label {
            let _ = sqlx::query(
                "UPDATE rootcx_system.integration_connections SET label = $1 WHERE id = $2"
            ).bind(l).bind(&conn_id).execute(&pool).await;
        }

        info!(integration_id = %pending.integration_id, user_id = %pending.user_id,
            connection_id = %conn_id, label = label.as_deref().unwrap_or(""), "credentials stored");

        if let Some((code, msg)) = try_auto_sync_connect(&rt, &pool, &secrets, &wm, &pending).await {
            return Ok(Html(callback_html(&pending.integration_id, Some((&code, &msg)))));
        }
    }

    Ok(Html(callback_html(&pending.integration_id, None)))
}

async fn try_auto_sync_connect(
    _rt: &SharedRuntime,
    pool: &sqlx::PgPool,
    secrets: &crate::secrets::SecretManager,
    wm: &crate::worker_manager::WorkerManager,
    pending: &Pending,
) -> Option<(String, String)> {
    let user_id: uuid::Uuid = pending.user_id.parse().ok()?;
    let email: String = sqlx::query_scalar("SELECT email FROM rootcx_system.users WHERE id = $1")
        .bind(user_id).fetch_optional(pool).await.ok()??;
    if email.is_empty() { return None; }

    let (config, user_credentials, effective_uid, _conn_id) = super::connections::resolve_credentials(
        secrets, pool, &pending.integration_id, &pending.user_id, None,
    ).await;
    let caller = Some(crate::ipc::RpcCaller { user_id: pending.user_id.clone(), email: email.clone(), effective_perms: None });

    match wm.rpc(
        &pending.integration_id, Uuid::new_v4().to_string(), "__integration".into(),
        json!({ "action": "sync_connect", "input": {}, "config": config, "userCredentials": user_credentials, "userId": effective_uid }),
        caller,
    ).await {
        Ok(res) if res.get("ok").and_then(|v| v.as_bool()) == Some(false) => {
            let err = res.get("error");
            let code = err.and_then(|e| e.get("code")).and_then(|v| v.as_str()).unwrap_or("UNKNOWN");
            let msg = err.and_then(|e| e.get("message")).and_then(|v| v.as_str()).unwrap_or("");
            warn!(integration_id = %pending.integration_id, user_id = %pending.user_id, code, "auto sync_connect failed: {}", msg);
            Some((code.to_owned(), msg.to_owned()))
        }
        Err(e) => {
            warn!(integration_id = %pending.integration_id, user_id = %pending.user_id, "auto sync_connect rpc error: {}", e);
            None
        }
        _ => None,
    }
}

fn callback_html(integration_id: &str, error: Option<(&str, &str)>) -> String {
    let escape = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;");
    let body = match error {
        Some((code, msg)) => format!(
            r#"<div style="text-align:center;max-width:560px;padding:24px"><h2 style="color:#ff6b6b;margin:0 0 12px">Connected, but sync failed</h2><p style="color:#aaa;margin:0 0 16px">{}</p><p style="font-family:monospace;background:#2a2a2a;padding:12px;border-radius:6px;font-size:13px;text-align:left;color:#ddd;word-break:break-word">[{}] {}</p><p style="color:#888;font-size:13px;margin-top:16px">You can close this tab and retry from the app settings.</p></div>"#,
            escape(integration_id), escape(code), escape(msg),
        ),
        None => format!(
            r#"<div style="text-align:center"><h2>Connected to {}!</h2><p>You can close this tab.</p></div>"#,
            escape(integration_id),
        ),
    };
    format!(
        r#"<!DOCTYPE html><html><body style="font-family:system-ui;display:flex;align-items:center;justify-content:center;min-height:100vh;margin:0;background:#1a1a1a;color:#fff">{}</body></html>"#,
        body,
    )
}

pub async fn status(
    identity: Identity, State(rt): State<SharedRuntime>,
    Path(integration_id): Path<String>,
    axum::extract::Query(qs): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = crate::routes::pool(&rt);
    if qs.contains_key("agent_app_id") {
        require_admin(&pool, identity.user_id).await?;
    }
    let target_user = resolve_target_user(&identity, &qs);

    // 'active' connections are usable; 'dead' ones had their credentials
    // rejected by the provider. `connected` reflects only live connections so a
    // silently-dead grant no longer reports as connected.
    let (active, dead): (i64, i64) = sqlx::query_as(
        "SELECT
            COUNT(*) FILTER (WHERE status = 'active'),
            COUNT(*) FILTER (WHERE status = 'dead')
         FROM rootcx_system.integration_connections
         WHERE integration_id = $1 AND user_id = $2 AND kind = 'direct'"
    )
    .bind(&integration_id)
    .bind(&target_user)
    .fetch_one(&pool).await.unwrap_or((0, 0));

    Ok(Json(json!({
        "connected": active > 0,
        "connectionCount": active,
        "deadCount": dead,
    })))
}

pub async fn submit_credentials(
    identity: Identity, State(rt): State<SharedRuntime>,
    Path(integration_id): Path<String>,
    Json(body): Json<JsonValue>,
) -> Result<Json<JsonValue>, ApiError> {
    let creds = body.get("credentials").ok_or_else(|| ApiError::BadRequest("missing credentials".into()))?;
    let (pool, secrets) = crate::routes::pool_and_secrets(&rt);
    let user_id = identity.user_id.to_string();

    let label = body.get("label").and_then(|v| v.as_str());
    // Reconnecting a known mailbox (same label) reuses its row and clears any
    // stale 'dead' flag; the credentials below overwrite the rejected ones.
    let conn_id = super::connections::upsert_connection(&pool, &integration_id, &user_id, label, None).await?;
    let conn_key = super::connections::credential_key(&conn_id);
    secrets.set(&pool, &integration_id, &conn_key, &creds.to_string()).await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(json!({ "message": "credentials stored", "connectionId": conn_id })))
}

pub async fn delegate(
    identity: Identity, State(rt): State<SharedRuntime>,
    Path(integration_id): Path<String>,
    Json(body): Json<JsonValue>,
) -> Result<Json<JsonValue>, ApiError> {
    let agent_app_id = body.get("agent_app_id").and_then(|v| v.as_str())
        .ok_or_else(|| ApiError::BadRequest("missing agent_app_id".into()))?;
    let source_user = identity.user_id.to_string();
    let agent_uid = crate::extensions::agents::agent_user_id(agent_app_id).to_string();

    let (pool, secrets) = crate::routes::pool_and_secrets(&rt);
    require_admin(&pool, identity.user_id).await?;

    let source_conn_id = super::connections::first_direct_connection(&pool, &integration_id, &source_user).await?
        .ok_or_else(|| ApiError::BadRequest("you must connect this integration first".into()))?;

    let delegate_conn_id = super::connections::create_connection(
        &pool, &integration_id, &agent_uid, None, "delegation", None,
    ).await?;
    let conn_key = super::connections::credential_key(&delegate_conn_id);
    let delegation = json!({ "_delegate": source_conn_id }).to_string();
    secrets.set(&pool, &integration_id, &conn_key, &delegation).await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    info!(integration_id, agent_app_id, source_user, "credentials delegated to agent");
    Ok(Json(json!({ "message": "delegated" })))
}

pub async fn disconnect(
    identity: Identity, State(rt): State<SharedRuntime>,
    Path(integration_id): Path<String>,
    axum::extract::Query(qs): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<JsonValue>, ApiError> {
    let (pool, secrets) = crate::routes::pool_and_secrets(&rt);
    if qs.contains_key("agent_app_id") {
        require_admin(&pool, identity.user_id).await?;
    }
    let target_user = resolve_target_user(&identity, &qs);

    let conn_ids: Vec<(String,)> = sqlx::query_as(
        "SELECT id FROM rootcx_system.integration_connections
         WHERE integration_id = $1 AND user_id = $2"
    )
    .bind(&integration_id)
    .bind(&target_user)
    .fetch_all(&pool).await?;

    for (conn_id,) in &conn_ids {
        let _ = secrets.delete(&pool, &integration_id, &super::connections::credential_key(conn_id)).await;
    }
    sqlx::query(
        "DELETE FROM rootcx_system.integration_connections
         WHERE integration_id = $1 AND user_id = $2"
    )
    .bind(&integration_id)
    .bind(&target_user)
    .execute(&pool).await?;

    Ok(Json(json!({ "message": "disconnected" })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::identity::Identity;

    fn identity(id: &str) -> Identity {
        Identity { user_id: Uuid::parse_str(id).unwrap(), email: String::new() }
    }

    #[test]
    fn resolve_target_user_no_agent() {
        let id = identity("550e8400-e29b-41d4-a716-446655440000");
        let qs = HashMap::new();
        assert_eq!(resolve_target_user(&id, &qs), "550e8400-e29b-41d4-a716-446655440000");
    }

    #[test]
    fn resolve_target_user_with_agent() {
        let id = identity("550e8400-e29b-41d4-a716-446655440000");
        let qs = HashMap::from([("agent_app_id".into(), "onboarding".into())]);
        let result = resolve_target_user(&id, &qs);
        assert_ne!(result, "550e8400-e29b-41d4-a716-446655440000");
        assert_eq!(result, resolve_target_user(&id, &qs));
    }

    #[test]
    fn resolve_target_user_different_agents_differ() {
        let id = identity("00000000-0000-0000-0000-000000000001");
        let a = resolve_target_user(&id, &HashMap::from([("agent_app_id".into(), "agent-a".into())]));
        let b = resolve_target_user(&id, &HashMap::from([("agent_app_id".into(), "agent-b".into())]));
        assert_ne!(a, b);
    }
}
