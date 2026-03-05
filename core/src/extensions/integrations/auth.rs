use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::Instant;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::Html;
use serde_json::{Value as JsonValue, json};
use tokio::sync::Mutex;
use tracing::info;
use uuid::Uuid;

use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::routes::SharedRuntime;

struct Pending {
    app_id: String,
    integration_id: String,
    user_id: String,
    created: Instant,
}

static PENDING: LazyLock<Mutex<HashMap<String, Pending>>> = LazyLock::new(Default::default);
const TTL_SECS: u64 = 600;

fn base_url(headers: &HeaderMap) -> String {
    let host = headers.get("host").and_then(|v| v.to_str().ok()).unwrap_or("localhost:9100");
    // Normalize 127.0.0.1 → localhost for consistent OAuth redirect URIs
    let host = if host.starts_with("127.0.0.1") { host.replacen("127.0.0.1", "localhost", 1) } else { host.to_string() };
    let scheme = headers.get("x-forwarded-proto").and_then(|v| v.to_str().ok()).unwrap_or("http");
    format!("{scheme}://{host}")
}

pub async fn start(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((app_id, integration_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Json<JsonValue>, ApiError> {
    let (pool, secrets) = crate::routes::pool_and_secrets(&rt).await?;
    let wm = crate::routes::wm(&rt).await?;

    let config = super::routes::fetch_config(&pool, &secrets, &app_id, &integration_id).await?;

    let nonce = Uuid::new_v4().to_string();
    let callback_url = format!("{}/api/v1/integrations/auth/callback", base_url(&headers));

    PENDING.lock().await.insert(nonce.clone(), Pending {
        app_id: app_id.clone(),
        integration_id: integration_id.clone(),
        user_id: identity.user_id.to_string(),
        created: Instant::now(),
    });

    let result = wm.rpc(
        &integration_id,
        Uuid::new_v4().to_string(),
        "__auth_start".into(),
        json!({ "config": config, "callbackUrl": callback_url, "state": nonce, "userId": identity.user_id.to_string() }),
        None,
    ).await.map_err(|e| ApiError::Internal(e.to_string()))?;

    info!(app_id, integration_id, user_id = %identity.user_id, "auth flow started");
    Ok(Json(result))
}

pub async fn callback(
    State(rt): State<SharedRuntime>,
    headers: HeaderMap,
    axum::extract::Query(params): axum::extract::Query<HashMap<String, String>>,
) -> Result<Html<String>, ApiError> {
    let state = params.get("state")
        .ok_or_else(|| ApiError::BadRequest("missing state parameter".into()))?;

    let pending = {
        let mut map = PENDING.lock().await;
        map.retain(|_, p| p.created.elapsed().as_secs() < TTL_SECS);
        map.remove(state)
    }.ok_or_else(|| ApiError::BadRequest("invalid or expired auth state".into()))?;

    let (pool, secrets) = crate::routes::pool_and_secrets(&rt).await?;
    let wm = crate::routes::wm(&rt).await?;

    let config = super::routes::fetch_config(&pool, &secrets, &pending.app_id, &pending.integration_id).await?;
    let callback_url = format!("{}/api/v1/integrations/auth/callback", base_url(&headers));

    let result = wm.rpc(
        &pending.integration_id,
        Uuid::new_v4().to_string(),
        "__auth_callback".into(),
        json!({ "config": config, "query": params, "userId": pending.user_id, "callbackUrl": callback_url }),
        None,
    ).await.map_err(|e| ApiError::Internal(e.to_string()))?;

    if let Some(credentials) = result.get("credentials") {
        let key = format!("_iuc.{}.{}", pending.integration_id, pending.user_id);
        secrets.set(&pool, &pending.app_id, &key, &credentials.to_string())
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
        info!(app_id = %pending.app_id, integration_id = %pending.integration_id, user_id = %pending.user_id, "user credentials stored");
    }

    Ok(Html(format!(
        r#"<!DOCTYPE html><html><body style="font-family:system-ui;display:flex;align-items:center;justify-content:center;height:100vh;margin:0;background:#1a1a1a;color:#fff"><div style="text-align:center"><h2>Connected to {}!</h2><p>You can close this tab.</p></div></body></html>"#,
        pending.integration_id,
    )))
}

pub async fn status(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((app_id, integration_id)): Path<(String, String)>,
) -> Result<Json<JsonValue>, ApiError> {
    let (pool, secrets) = crate::routes::pool_and_secrets(&rt).await?;
    let key = format!("_iuc.{}.{}", integration_id, identity.user_id);
    let connected = secrets.get(&pool, &app_id, &key).await
        .map_err(|e| ApiError::Internal(e.to_string()))?
        .is_some();
    Ok(Json(json!({ "connected": connected })))
}

pub async fn submit_credentials(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((app_id, integration_id)): Path<(String, String)>,
    Json(body): Json<JsonValue>,
) -> Result<Json<JsonValue>, ApiError> {
    let credentials = body.get("credentials")
        .ok_or_else(|| ApiError::BadRequest("missing credentials".into()))?;

    let (pool, secrets) = crate::routes::pool_and_secrets(&rt).await?;
    let key = format!("_iuc.{}.{}", integration_id, identity.user_id);
    secrets.set(&pool, &app_id, &key, &credentials.to_string())
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    info!(app_id, integration_id, user_id = %identity.user_id, "user credentials submitted");
    Ok(Json(json!({ "message": "credentials stored" })))
}

pub async fn disconnect(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((app_id, integration_id)): Path<(String, String)>,
) -> Result<Json<JsonValue>, ApiError> {
    let (pool, secrets) = crate::routes::pool_and_secrets(&rt).await?;
    let key = format!("_iuc.{}.{}", integration_id, identity.user_id);
    let _ = secrets.delete(&pool, &app_id, &key).await;
    Ok(Json(json!({ "message": "disconnected" })))
}
