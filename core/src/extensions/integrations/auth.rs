use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::Html;
use hmac::{Hmac, Mac};
use serde_json::{Value as JsonValue, json};
use sha2::{Digest, Sha256};
use base64::Engine;
use serde::{Serialize, Deserialize};
use tracing::{info, warn};
use crate::governance::authority::require_admin;
use uuid::Uuid;

use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::routes::SharedRuntime;

type HmacSha256 = Hmac<Sha256>;

#[derive(Serialize, Deserialize)]
struct Pending { integration_id: String, user_id: String, config_id: Option<String>, callback_url: String, code_verifier: String }

pub(crate) async fn bootstrap(pool: &sqlx::PgPool) -> Result<(), crate::RuntimeError> {
    sqlx::query("CREATE TABLE IF NOT EXISTS rootcx_system.integration_oauth_states (
        state_hash TEXT PRIMARY KEY, nonce BYTEA NOT NULL, ciphertext BYTEA NOT NULL,
        expires_at TIMESTAMPTZ NOT NULL DEFAULT now() + interval '10 minutes')")
        .execute(pool).await.map_err(crate::RuntimeError::Schema)?;
    sqlx::query("CREATE INDEX IF NOT EXISTS integration_oauth_states_expiry ON rootcx_system.integration_oauth_states (expires_at)")
        .execute(pool).await.map_err(crate::RuntimeError::Schema)?;
    sqlx::query("CREATE TABLE IF NOT EXISTS rootcx_system.integration_oauth_attempts (
        id TEXT PRIMARY KEY, integration_id TEXT NOT NULL, user_id TEXT NOT NULL,
        connection_id TEXT, status TEXT NOT NULL DEFAULT 'pending',
        expires_at TIMESTAMPTZ NOT NULL DEFAULT now() + interval '10 minutes')")
        .execute(pool).await.map_err(crate::RuntimeError::Schema)?;
    sqlx::query("CREATE INDEX IF NOT EXISTS integration_oauth_attempts_expiry ON rootcx_system.integration_oauth_attempts (expires_at)")
        .execute(pool).await.map_err(crate::RuntimeError::Schema)?;
    Ok(())
}

fn state_hash(state: &str) -> String { hex::encode(Sha256::digest(state.as_bytes())) }

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

    let code_verifier = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let code_challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(code_verifier.as_bytes()));
    let pending = Pending {
        integration_id: integration_id.clone(), user_id: identity.user_id.to_string(),
        config_id, callback_url: cb.clone(), code_verifier,
    };
    let (iv, ciphertext) = secrets.encrypt(&serde_json::to_string(&pending).map_err(|e| ApiError::Internal(e.to_string()))?)
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    sqlx::query("DELETE FROM rootcx_system.integration_oauth_states WHERE expires_at <= now()")
        .execute(&pool).await?;
    sqlx::query("INSERT INTO rootcx_system.integration_oauth_states (state_hash, nonce, ciphertext) VALUES ($1, $2, $3)")
        .bind(state_hash(&nonce)).bind(iv).bind(ciphertext).execute(&pool).await?;

    let mut result = if super::managed::enabled(&integration_id) {
        super::managed::start(&integration_id, &config, &cb, &state, &code_challenge)
    } else { wm.rpc(
        &integration_id, Uuid::new_v4().to_string(), "__auth_start".into(),
        json!({ "config": config, "callbackUrl": cb, "state": state, "codeChallenge": code_challenge, "userId": identity.user_id }),
        None,
    ).await.map_err(|e| ApiError::Internal(e.to_string()))? };

    sqlx::query("DELETE FROM rootcx_system.integration_oauth_attempts WHERE expires_at <= now()")
        .execute(&pool).await?;
    sqlx::query("INSERT INTO rootcx_system.integration_oauth_attempts (id, integration_id, user_id) VALUES ($1, $2, $3)")
        .bind(state_hash(&nonce)).bind(&integration_id).bind(identity.user_id.to_string()).execute(&pool).await?;
    result["attemptId"] = json!(nonce);
    info!(integration_id, user_id = %identity.user_id, "auth flow started");
    Ok(Json(result))
}

async fn consume_state(pool: &sqlx::PgPool, secrets: &crate::secrets::SecretManager, nonce: &str) -> Result<Pending, ApiError> {
    let stored: Option<(Vec<u8>, Vec<u8>)> = sqlx::query_as(
        "DELETE FROM rootcx_system.integration_oauth_states WHERE state_hash = $1 AND expires_at > now() RETURNING nonce, ciphertext"
    ).bind(state_hash(nonce)).fetch_optional(pool).await?;
    let (iv, ciphertext) = stored.ok_or_else(|| ApiError::BadRequest("invalid or expired auth state".into()))?;
    serde_json::from_str(&secrets.decrypt(&iv, &ciphertext)
        .map_err(|e| ApiError::Internal(e.to_string()))?).map_err(|e| ApiError::Internal(e.to_string()))
}

pub(super) fn callback_error_category(error: &ApiError) -> &'static str {
    match error {
        ApiError::BadRequest(message) => match message.as_str() {
            "Provider returned incomplete authorization data" => "incomplete_credentials",
            "Provider returned invalid token lifetime" => "invalid_lifetime",
            "Provider authorization or permissions rejected" => "provider_rejected",
            "Provider temporarily unavailable" | "Provider response interrupted" => "provider_unavailable",
            _ => "invalid_request",
        },
        ApiError::Unauthorized(_) | ApiError::Forbidden(_) => "permission_denied",
        ApiError::Internal(_) => "internal_failure",
        _ => "callback_failure",
    }
}

pub async fn callback(
    State(rt): State<SharedRuntime>,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Html<String>, ApiError> {
    let state = params.get("state").ok_or_else(|| ApiError::BadRequest("missing state".into()))?;
    let (pool, secrets) = crate::routes::pool_and_secrets(&rt);
    let pending = consume_state(&pool, &secrets, extract_nonce(state)).await?;
    let attempt_hash = state_hash(extract_nonce(state));
    let outcome = finish_callback(&rt, &pool, &secrets, &pending, &params, &attempt_hash).await;
    let status = if outcome.is_ok() { "connected" } else if params.get("error").map(String::as_str) == Some("access_denied") { "cancelled" } else { "failed" };
    sqlx::query("UPDATE rootcx_system.integration_oauth_attempts SET status = $2 WHERE id = $1")
        .bind(&attempt_hash).bind(status).execute(&pool).await?;
    if let Err(ref failure) = outcome {
        warn!(integration_id = %pending.integration_id, stage = "callback", category = callback_error_category(failure), "OAuth authorization was not completed");
        return Ok(Html(callback_html(&pending.integration_id, &pending.callback_url, Some((status, "Authorization was not completed")))));
    }
    Ok(Html(callback_html(&pending.integration_id, &pending.callback_url, None)))
}

pub async fn attempt_status(
    identity: Identity, State(rt): State<SharedRuntime>,
    Path((integration_id, attempt_id)): Path<(String, String)>,
) -> Result<Json<JsonValue>, ApiError> {
    let row: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT status, connection_id FROM rootcx_system.integration_oauth_attempts
         WHERE id = $1 AND integration_id = $2 AND user_id = $3 AND expires_at > now()")
        .bind(state_hash(&attempt_id)).bind(integration_id).bind(identity.user_id.to_string())
        .fetch_optional(&crate::routes::pool(&rt)).await?;
    let (status, connection) = row.ok_or_else(|| ApiError::NotFound("auth attempt not found or expired".into()))?;
    Ok(Json(json!({ "status": status, "connectionId": if status == "connected" { connection } else { None } })))
}

pub async fn verify_managed_connection(
    identity: Identity, State(rt): State<SharedRuntime>,
    Path((integration_id, connection_id)): Path<(String, String)>,
) -> Result<Json<JsonValue>, ApiError> {
    if !super::managed::enabled(&integration_id) { return Err(ApiError::NotFound("managed connector not enabled".into())); }
    let (pool, secrets) = crate::routes::pool_and_secrets(&rt);
    super::connections::verify_owner(&pool, &connection_id, &integration_id, &identity).await?;
    let (_, credentials, _, _, _) = super::connections::resolve_by_connection_id(&secrets, &pool, &integration_id, &connection_id, &identity.user_id.to_string())
        .await.map_err(|_| ApiError::BadRequest("Reconnect your account".into()))?;
    super::managed::verify(&integration_id, &credentials).await?;
    Ok(Json(json!({"verified": true})))
}

async fn finish_callback(
    rt: &SharedRuntime, pool: &sqlx::PgPool, secrets: &crate::secrets::SecretManager,
    pending: &Pending, params: &HashMap<String, String>, attempt_hash: &str,
) -> Result<(), ApiError> {
    let wm = crate::routes::wm(&rt);
    let config = super::routes::resolve_config_scoped(&pool, &secrets, &pending.integration_id, pending.config_id.as_deref()).await?;

    let result = if super::managed::enabled(&pending.integration_id) {
        super::managed::callback(&pending.integration_id, &config, params, &pending.callback_url, &pending.code_verifier).await?
    } else { wm.rpc(
        &pending.integration_id, Uuid::new_v4().to_string(), "__auth_callback".into(),
        json!({ "config": config, "query": params, "userId": pending.user_id, "callbackUrl": pending.callback_url, "codeVerifier": pending.code_verifier }),
        None,
    ).await.map_err(|e| ApiError::Internal(e.to_string()))? };

    let creds = result.get("credentials").filter(|v| v.is_object())
        .ok_or_else(|| ApiError::BadRequest("provider did not return verified credentials".into()))?;
    {
        let label = result.get("label").and_then(|v| v.as_str()).map(|s| s.to_string());

        let conn_id = super::connections::upsert_connection(
            &pool, &secrets, &pending.integration_id, &pending.user_id, label.as_deref(), pending.config_id.as_deref(), creds,
        ).await?;

        sqlx::query("UPDATE rootcx_system.integration_oauth_attempts SET connection_id = $2 WHERE id = $1")
            .bind(attempt_hash).bind(&conn_id).execute(pool).await?;
        info!(integration_id = %pending.integration_id, user_id = %pending.user_id,
            connection_id = %conn_id, label = label.as_deref().unwrap_or(""), "credentials stored");

        if let Some((code, msg)) = try_auto_sync_connect(&pool, &secrets, &wm, &pending, &conn_id).await {
            return Err(ApiError::BadRequest(format!("{code}: {msg}")));
        }
    }

    Ok(())
}

async fn try_auto_sync_connect(
    pool: &sqlx::PgPool,
    secrets: &crate::secrets::SecretManager,
    wm: &crate::worker_manager::WorkerManager,
    pending: &Pending,
    conn_id: &str,
) -> Option<(String, String)> {
    let manifest = super::routes::get_integration_manifest(pool, &pending.integration_id).await.ok()?;
    if !manifest.get("actions").and_then(|v| v.as_array()).is_some_and(|actions| actions.iter().any(|a| a.get("id").and_then(|v| v.as_str()) == Some("sync_connect"))) { return None; }
    let user_id: uuid::Uuid = pending.user_id.parse().ok()?;
    let caller = Some(crate::principal::resolve_caller_pinned(pool, user_id, conn_id.to_string()).await?);

    let (config, user_credentials, effective_uid, resolved_conn, credential_version) = match super::connections::resolve_by_connection_id(
        secrets, pool, &pending.integration_id, conn_id, &pending.user_id,
    ).await {
        Ok(r) => r,
        // Dead-flagging happened inside the renewal; surface the cause to the page.
        Err(e) => return Some(("unauthorized".into(), e.message().to_string())),
    };

    match wm.rpc(
        &pending.integration_id, Uuid::new_v4().to_string(), "__integration".into(),
        json!({ "action": "sync_connect", "input": {}, "config": config, "userCredentials": user_credentials, "userId": effective_uid, "connectionId": resolved_conn.clone() }),
        caller,
    ).await {
        Ok(res) if res.get("ok").and_then(|v| v.as_bool()) == Some(false) => {
            // The one send path that skipped the post-call funnel. It is also the
            // first call a new connection ever makes, so a credential rejected here
            // was reported as connected until the next action happened to fail.
            super::connections::flag_if_auth_failed(
                pool, secrets, &pending.integration_id, resolved_conn.as_deref(), credential_version.as_ref(), &res,
            ).await;
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

fn callback_html(integration_id: &str, callback_url: &str, error: Option<(&str, &str)>) -> String {
    if integration_id == "google_drive" && super::managed::enabled(integration_id) {
        // Use the server-owned OAuth callback origin, never a user-provided redirect.
        let return_url = url::Url::parse(callback_url)
            .ok().filter(|url| matches!(url.scheme(), "http" | "https"))
            .and_then(|url| url.join("/shapp/companies").ok())
            .map(|url| url.to_string()).unwrap_or_else(|| "/".into());
        let return_url = return_url.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;").replace('>', "&gt;");
        let encode = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
        let cancelled = error.is_some();
        return include_str!("callback-assets/connected.html")
            .replace("{{TITLE}}", if cancelled { "Connexion non terminée" } else { "Google Drive connecté" })
            .replace("{{HEADING}}", if cancelled { "Google Drive n’est pas encore connecté." } else { "Google Drive rejoint votre espace SHAPP." })
            .replace("{{DESCRIPTION}}", if cancelled { "La connexion n’a pas été activée. Retournez dans votre application pour réessayer." } else { "Votre compte est connecté. Retrouvez-le dans les intégrations de votre application." })
            .replace("{{RESULT}}", if cancelled { "Connexion non terminée" } else { "Connexion réussie" })
            .replace("{{SUMMARY}}", if cancelled { "Aucun nouvel accès Google Drive n’a été activé dans SHAPP." } else { "Google Drive est maintenant connecté à SHAPP." })
            .replace("{{STATUS}}", if cancelled { "À reconnecter" } else { "Connecté" })
            .replace("{{ICON}}", if cancelled { "M12 7v6m0 4h.01" } else { "m5 12 4 4 10-10" })
            .replace("{{MARK}}", if cancelled { "·" } else { "✓" })
            .replace("{{SHAPPY}}", &encode(include_bytes!("callback-assets/shappy.webp")))
            .replace("{{DRIVE}}", &encode(include_bytes!("callback-assets/google-drive.png")))
            .replace("{{FONT}}", &encode(include_bytes!("callback-assets/inter-latin.woff2")))
            .replace("{{THEME}}", include_str!("callback-assets/shapp-theme.css"))
            .replace("{{RETURN_URL}}", &return_url);
    }
    let escape = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;");
    let body = match error {
        Some((code, msg)) => format!(
            r#"<div style="text-align:center;max-width:560px;padding:24px"><h2 style="color:#ff6b6b;margin:0 0 12px">Authorization was not completed</h2><p style="color:#aaa;margin:0 0 16px">{}</p><p style="font-family:monospace;background:#2a2a2a;padding:12px;border-radius:6px;font-size:13px;text-align:left;color:#ddd;word-break:break-word">[{}] {}</p><p style="color:#888;font-size:13px;margin-top:16px">You can close this tab and retry from the app settings.</p></div>"#,
            escape(integration_id), escape(code), escape(msg),
        ),
        None => format!(
            r#"<div style="text-align:center"><h2>Connected to {}!</h2><p>You can close this tab.</p></div>"#,
            escape(integration_id),
        ),
    };
    format!(
        r#"<!DOCTYPE html><html><body style="font-family:system-ui;display:flex;align-items:center;justify-content:center;min-height:100vh;margin:0;background:#1a1a1a;color:#fff">{}<script>window.close()</script></body></html>"#,
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
    super::managed::reject_override(&integration_id)?;
    let creds = body.get("credentials").ok_or_else(|| ApiError::BadRequest("missing credentials".into()))?;
    let (pool, secrets) = crate::routes::pool_and_secrets(&rt);
    let user_id = identity.user_id.to_string();

    let manifest = super::routes::get_integration_manifest(&pool, &integration_id).await?;
    let verified = if manifest.pointer("/userAuth/plugin/protocolVersion").is_some() {
        Some(crate::routes::wm(&rt).rpc(&integration_id, Uuid::new_v4().to_string(), "__auth_credentials".into(),
            json!({ "credentials": creds, "userId": user_id }), None).await
            .map_err(|_| ApiError::BadRequest("provider could not verify these credentials".into()))?)
    } else { None };
    let creds = match &verified {
        Some(result) => result.get("credentials").filter(|v| v.is_object())
            .ok_or_else(|| ApiError::BadRequest("provider did not verify credentials".into()))?,
        None => creds,
    };
    let label = verified.as_ref().and_then(|v| v.get("label")).and_then(|v| v.as_str())
        .or_else(|| body.get("label").and_then(|v| v.as_str()));
    // Reactivation and credential replacement must become visible together.
    let conn_id = super::connections::upsert_connection(&pool, &secrets, &integration_id, &user_id, label, None, creds).await?;

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

    #[test]
    fn callback_diagnostics_never_return_provider_error_content() {
        for (error, category) in [
            (ApiError::BadRequest("Provider returned incomplete authorization data".into()), "incomplete_credentials"),
            (ApiError::BadRequest("access_token=private-provider-body".into()), "invalid_request"),
            (ApiError::Internal("client_secret=private-config".into()), "internal_failure"),
        ] {
            assert_eq!(callback_error_category(&error), category);
        }
    }

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
    #[tokio::test]
    async fn oauth_state_is_single_use_across_connections_and_rejects_expired_callbacks() {
        let pool = crate::extensions::test_db::pool().await;
        bootstrap(&pool).await.unwrap();
        let secrets = crate::secrets::SecretManager::with_key(&[0x4A; 32]);
        for expired in [false, true] {
            let nonce = Uuid::new_v4().to_string();
            let pending = Pending { integration_id: "notion".into(), user_id: "owner".into(), config_id: None, callback_url: "https://app.example/callback".into(), code_verifier: "private-verifier".into() };
            let (iv, cipher) = secrets.encrypt(&serde_json::to_string(&pending).unwrap()).unwrap();
            sqlx::query("INSERT INTO rootcx_system.integration_oauth_states (state_hash, nonce, ciphertext, expires_at) VALUES ($1, $2, $3, now() + $4 * interval '1 second')")
                .bind(state_hash(&nonce)).bind(iv).bind(cipher).bind(if expired { -1.0_f64 } else { 60.0_f64 }).execute(&pool).await.unwrap();
            let (a, b) = tokio::join!(consume_state(&pool, &secrets, &nonce), consume_state(&pool, &secrets, &nonce));
            assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), if expired { 0 } else { 1 });
            if let Ok(pending) = a.or(b) { assert_eq!(pending.code_verifier, "private-verifier"); assert_eq!(pending.user_id, "owner"); }
        }
    }

}
