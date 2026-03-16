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

struct Pending { app_id: String, integration_id: String, user_id: String, created: Instant }

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

fn iuc_key(integration_id: &str, user_id: &str) -> String {
    format!("_iuc.{integration_id}.{user_id}")
}

pub async fn start(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((app_id, integration_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<JsonValue>, ApiError> {
    let (pool, secrets) = crate::routes::pool_and_secrets(&rt).await?;
    let wm = crate::routes::wm(&rt).await?;
    let config = super::routes::resolve_config(&pool, &secrets, &integration_id).await?;

    let nonce = Uuid::new_v4().to_string();
    let (cb, state) = build_callback_and_state(&headers, &nonce);

    PENDING.lock().await.insert(nonce, Pending {
        app_id, integration_id: integration_id.clone(), user_id: identity.user_id.to_string(), created: Instant::now(),
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

    let (pool, secrets) = crate::routes::pool_and_secrets(&rt).await?;
    let wm = crate::routes::wm(&rt).await?;
    let config = super::routes::resolve_config(&pool, &secrets, &pending.integration_id).await?;

    let result = wm.rpc(
        &pending.integration_id, Uuid::new_v4().to_string(), "__auth_callback".into(),
        json!({ "config": config, "query": params, "userId": pending.user_id, "callbackUrl": resolve_callback_url(&headers) }),
        None,
    ).await.map_err(|e| ApiError::Internal(e.to_string()))?;

    if let Some(creds) = result.get("credentials") {
        let key = iuc_key(&pending.integration_id, &pending.user_id);
        secrets.set(&pool, &pending.app_id, &key, &creds.to_string()).await.map_err(|e| ApiError::Internal(e.to_string()))?;
        info!(integration_id = %pending.integration_id, user_id = %pending.user_id, "user credentials stored");
    }

    Ok(Html(format!(
        r#"<!DOCTYPE html><html><body style="font-family:system-ui;display:flex;align-items:center;justify-content:center;height:100vh;margin:0;background:#1a1a1a;color:#fff"><div style="text-align:center"><h2>Connected to {}!</h2><p>You can close this tab.</p></div></body></html>"#,
        pending.integration_id,
    )))
}

pub async fn status(
    identity: Identity, State(rt): State<SharedRuntime>,
    Path((app_id, integration_id)): Path<(String, String)>,
) -> Result<Json<JsonValue>, ApiError> {
    let (pool, secrets) = crate::routes::pool_and_secrets(&rt).await?;
    let connected = secrets.get(&pool, &app_id, &iuc_key(&integration_id, &identity.user_id.to_string())).await
        .map_err(|e| ApiError::Internal(e.to_string()))?.is_some();
    Ok(Json(json!({ "connected": connected })))
}

pub async fn submit_credentials(
    identity: Identity, State(rt): State<SharedRuntime>,
    Path((app_id, integration_id)): Path<(String, String)>,
    Json(body): Json<JsonValue>,
) -> Result<Json<JsonValue>, ApiError> {
    let creds = body.get("credentials").ok_or_else(|| ApiError::BadRequest("missing credentials".into()))?;
    let (pool, secrets) = crate::routes::pool_and_secrets(&rt).await?;
    secrets.set(&pool, &app_id, &iuc_key(&integration_id, &identity.user_id.to_string()), &creds.to_string()).await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(json!({ "message": "credentials stored" })))
}

pub async fn disconnect(
    identity: Identity, State(rt): State<SharedRuntime>,
    Path((app_id, integration_id)): Path<(String, String)>,
) -> Result<Json<JsonValue>, ApiError> {
    let (pool, secrets) = crate::routes::pool_and_secrets(&rt).await?;
    let _ = secrets.delete(&pool, &app_id, &iuc_key(&integration_id, &identity.user_id.to_string())).await;
    Ok(Json(json!({ "message": "disconnected" })))
}
