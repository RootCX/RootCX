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
use tracing::info;
use uuid::Uuid;

use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::routes::SharedRuntime;

type HmacSha256 = Hmac<Sha256>;

struct Pending { integration_id: String, user_id: String, created: Instant }

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

const DELEGATE_KEY: &str = "_delegate";

pub(crate) fn iuc_key(integration_id: &str, user_id: &str) -> String {
    format!("_iuc.{integration_id}.{user_id}")
}

fn resolve_target_user(identity: &Identity, qs: &HashMap<String, String>) -> String {
    qs.get("agent_app_id")
        .map(|id| crate::extensions::agents::agent_user_id(id).to_string())
        .unwrap_or_else(|| identity.user_id.to_string())
}

enum CredentialValue {
    Direct(JsonValue),
    Delegated(String),
}

fn parse_credential_value(raw: &str) -> CredentialValue {
    let val: JsonValue = serde_json::from_str(raw).unwrap_or(JsonValue::Null);
    match val.get(DELEGATE_KEY).and_then(|v| v.as_str()) {
        Some(delegate) => CredentialValue::Delegated(delegate.to_string()),
        None => CredentialValue::Direct(val),
    }
}

/// Resolve credentials, following delegation if present. Returns (credentials, effective_user_id).
pub(crate) async fn resolve_credentials(
    secrets: &crate::secrets::SecretManager, pool: &sqlx::PgPool,
    integration_id: &str, user_id: &str,
) -> (JsonValue, String) {
    let key = iuc_key(integration_id, user_id);
    match secrets.get(pool, integration_id, &key).await {
        Ok(Some(raw)) => match parse_credential_value(&raw) {
            CredentialValue::Delegated(delegate) => {
                let dk = iuc_key(integration_id, &delegate);
                let creds = match secrets.get(pool, integration_id, &dk).await {
                    Ok(Some(r)) => serde_json::from_str(&r).unwrap_or(JsonValue::Null),
                    _ => JsonValue::Null,
                };
                (creds, delegate)
            }
            CredentialValue::Direct(val) => (val, user_id.to_string()),
        }
        _ => (JsonValue::Null, user_id.to_string()),
    }
}

pub async fn start(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(integration_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<JsonValue>, ApiError> {
    let (pool, secrets) = crate::routes::pool_and_secrets(&rt);
    let wm = crate::routes::wm(&rt);
    let config = super::routes::resolve_config(&pool, &secrets, &integration_id).await?;

    let nonce = Uuid::new_v4().to_string();
    let (cb, state) = build_callback_and_state(&headers, &nonce);

    PENDING.lock().await.insert(nonce, Pending {
        integration_id: integration_id.clone(), user_id: identity.user_id.to_string(), created: Instant::now(),
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
    headers: HeaderMap,
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
    let config = super::routes::resolve_config(&pool, &secrets, &pending.integration_id).await?;

    let result = wm.rpc(
        &pending.integration_id, Uuid::new_v4().to_string(), "__auth_callback".into(),
        json!({ "config": config, "query": params, "userId": pending.user_id, "callbackUrl": resolve_callback_url(&headers) }),
        None,
    ).await.map_err(|e| ApiError::Internal(e.to_string()))?;

    if let Some(creds) = result.get("credentials") {
        let key = iuc_key(&pending.integration_id, &pending.user_id);
        secrets.set(&pool, &pending.integration_id, &key, &creds.to_string()).await.map_err(|e| ApiError::Internal(e.to_string()))?;
        info!(integration_id = %pending.integration_id, user_id = %pending.user_id, "user credentials stored");
    }

    Ok(Html(format!(
        r#"<!DOCTYPE html><html><body style="font-family:system-ui;display:flex;align-items:center;justify-content:center;height:100vh;margin:0;background:#1a1a1a;color:#fff"><div style="text-align:center"><h2>Connected to {}!</h2><p>You can close this tab.</p></div></body></html>"#,
        pending.integration_id,
    )))
}

pub async fn status(
    identity: Identity, State(rt): State<SharedRuntime>,
    Path(integration_id): Path<String>,
    axum::extract::Query(qs): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<JsonValue>, ApiError> {
    let target_user = resolve_target_user(&identity, &qs);
    let (pool, secrets) = crate::routes::pool_and_secrets(&rt);
    let raw = secrets.get(&pool, &integration_id, &iuc_key(&integration_id, &target_user)).await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    match raw {
        Some(val) => {
            let parsed: JsonValue = serde_json::from_str(&val).unwrap_or(JsonValue::Null);
            if let Some(delegate_uid) = parsed.get(DELEGATE_KEY).and_then(|v| v.as_str()) {
                let email: Option<String> = sqlx::query_scalar(
                    "SELECT email FROM rootcx_system.users WHERE id = $1"
                ).bind(uuid::Uuid::parse_str(delegate_uid).unwrap_or_default())
                .fetch_optional(&pool).await.ok().flatten();
                Ok(Json(json!({ "connected": true, "delegatedFrom": email })))
            } else {
                Ok(Json(json!({ "connected": true })))
            }
        }
        None => Ok(Json(json!({ "connected": false }))),
    }
}

pub async fn submit_credentials(
    identity: Identity, State(rt): State<SharedRuntime>,
    Path(integration_id): Path<String>,
    Json(body): Json<JsonValue>,
) -> Result<Json<JsonValue>, ApiError> {
    let creds = body.get("credentials").ok_or_else(|| ApiError::BadRequest("missing credentials".into()))?;
    let (pool, secrets) = crate::routes::pool_and_secrets(&rt);
    secrets.set(&pool, &integration_id, &iuc_key(&integration_id, &identity.user_id.to_string()), &creds.to_string()).await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(json!({ "message": "credentials stored" })))
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

    let source_key = iuc_key(&integration_id, &source_user);
    if secrets.get(&pool, &integration_id, &source_key).await
        .map_err(|e| ApiError::Internal(e.to_string()))?.is_none()
    {
        return Err(ApiError::BadRequest("you must connect this integration first".into()));
    }

    let key = iuc_key(&integration_id, &agent_uid);
    let delegation = json!({ DELEGATE_KEY: source_user }).to_string();
    secrets.set(&pool, &integration_id, &key, &delegation).await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    info!(integration_id, agent_app_id, source_user, "credentials delegated to agent");
    Ok(Json(json!({ "message": "delegated" })))
}

pub async fn disconnect(
    identity: Identity, State(rt): State<SharedRuntime>,
    Path(integration_id): Path<String>,
    axum::extract::Query(qs): axum::extract::Query<HashMap<String, String>>,
) -> Result<Json<JsonValue>, ApiError> {
    let target_user = resolve_target_user(&identity, &qs);
    let (pool, secrets) = crate::routes::pool_and_secrets(&rt);
    let _ = secrets.delete(&pool, &integration_id, &iuc_key(&integration_id, &target_user)).await;
    Ok(Json(json!({ "message": "disconnected" })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;
    use crate::auth::identity::Identity;

    fn identity(id: &str) -> Identity {
        Identity { user_id: Uuid::parse_str(id).unwrap(), email: String::new() }
    }

    // -- parse_credential_value --

    #[test]
    fn parse_direct_credentials() {
        let raw = r#"{"accessToken":"tok","refreshToken":"ref"}"#;
        match parse_credential_value(raw) {
            CredentialValue::Direct(val) => assert_eq!(val["accessToken"], "tok"),
            CredentialValue::Delegated(_) => panic!("expected Direct"),
        }
    }

    #[test]
    fn parse_delegation() {
        let uid = "550e8400-e29b-41d4-a716-446655440000";
        let raw = format!(r#"{{"_delegate":"{uid}"}}"#);
        match parse_credential_value(&raw) {
            CredentialValue::Delegated(id) => assert_eq!(id, uid),
            CredentialValue::Direct(_) => panic!("expected Delegated"),
        }
    }

    #[test]
    fn parse_managed_credentials_not_delegation() {
        let raw = r#"{"managed":true}"#;
        match parse_credential_value(raw) {
            CredentialValue::Direct(val) => assert_eq!(val["managed"], true),
            CredentialValue::Delegated(_) => panic!("managed flag should not be treated as delegation"),
        }
    }

    #[test]
    fn parse_invalid_json_falls_back_to_direct_null() {
        match parse_credential_value("not json") {
            CredentialValue::Direct(val) => assert!(val.is_null()),
            CredentialValue::Delegated(_) => panic!("invalid JSON should not delegate"),
        }
    }

    #[test]
    fn parse_empty_string_falls_back_to_direct_null() {
        match parse_credential_value("") {
            CredentialValue::Direct(val) => assert!(val.is_null()),
            CredentialValue::Delegated(_) => panic!("empty should not delegate"),
        }
    }

    // -- resolve_target_user --

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
        assert_ne!(result, "550e8400-e29b-41d4-a716-446655440000", "should use agent UUID, not human");
        // Deterministic: same input = same output
        assert_eq!(result, resolve_target_user(&id, &qs));
    }

    #[test]
    fn resolve_target_user_agent_is_deterministic_across_calls() {
        let id = identity("00000000-0000-0000-0000-000000000001");
        let qs1 = HashMap::from([("agent_app_id".into(), "my-agent".into())]);
        let qs2 = HashMap::from([("agent_app_id".into(), "my-agent".into())]);
        assert_eq!(resolve_target_user(&id, &qs1), resolve_target_user(&id, &qs2));
    }

    #[test]
    fn resolve_target_user_different_agents_differ() {
        let id = identity("00000000-0000-0000-0000-000000000001");
        let a = resolve_target_user(&id, &HashMap::from([("agent_app_id".into(), "agent-a".into())]));
        let b = resolve_target_user(&id, &HashMap::from([("agent_app_id".into(), "agent-b".into())]));
        assert_ne!(a, b);
    }

    // -- delegate writes source_user, not arbitrary user --

    #[test]
    fn delegate_key_format() {
        let delegation = json!({ DELEGATE_KEY: "user-123" }).to_string();
        match parse_credential_value(&delegation) {
            CredentialValue::Delegated(uid) => assert_eq!(uid, "user-123"),
            _ => panic!("delegation JSON should parse as Delegated"),
        }
    }
}
