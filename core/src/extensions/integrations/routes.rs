use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use serde_json::{Value as JsonValue, json};
use uuid::Uuid;

use base64::Engine;

use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::extensions::rbac::policy::resolve_permissions;
use crate::routes::{self, SharedRuntime};

const PLATFORM_SCOPE: &str = "_platform";

pub(super) fn platform_secret_map(manifest: &JsonValue) -> Vec<(String, String)> {
    let Some(props) = manifest.pointer("/configSchema/properties").and_then(|v| v.as_object()) else {
        return vec![];
    };
    props.iter().filter_map(|(field, def)| {
        def.get("platformSecret").and_then(|v| v.as_str()).map(|key| (field.clone(), key.to_string()))
    }).collect()
}

pub(crate) async fn query_installed_integrations(pool: &sqlx::PgPool) -> Result<Vec<JsonValue>, sqlx::Error> {
    let rows: Vec<(String, String, String, Option<JsonValue>)> = sqlx::query_as(
        "SELECT id, name, version, manifest FROM rootcx_system.apps
         WHERE manifest->>'type' = 'integration' AND status = 'installed' ORDER BY name",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(|(id, name, version, m)| {
        let m = m.unwrap_or(JsonValue::Null);
        json!({
            "id": id, "name": name, "version": version,
            "description": m.get("description").and_then(|v| v.as_str()).unwrap_or(""),
            "actions": m.get("actions").unwrap_or(&json!([])),
            "configSchema": m.get("configSchema"),
            "userAuth": m.get("userAuth"),
            "webhooks": m.get("webhooks").unwrap_or(&json!([])),
            "instructions": m.get("instructions"),
        })
    }).collect())
}

pub async fn list_integrations(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
) -> Result<Json<Vec<JsonValue>>, ApiError> {
    let (pool, secrets) = routes::pool_and_secrets(&rt);
    let mut items = query_installed_integrations(&pool).await?;
    for item in &mut items {
        let map = platform_secret_map(item);
        let configured = is_configured(&pool, &secrets, item, &map).await?;
        item.as_object_mut().unwrap().insert("configured".into(), json!(configured));
    }
    Ok(Json(items))
}

pub async fn save_platform_config(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(integration_id): Path<String>,
    Json(config): Json<JsonValue>,
) -> Result<Json<JsonValue>, ApiError> {
    let (pool, secrets) = routes::pool_and_secrets(&rt);
    let manifest = get_integration_manifest(&pool, &integration_id).await?;
    let secret_map = platform_secret_map(&manifest);
    save_config_secrets(&pool, &secrets, &secret_map, &config).await?;

    let wm = routes::wm(&rt);
    let full_config = fetch_config(&pool, &secrets, &secret_map).await?;
    if let Ok(result) = wm.rpc(
        &integration_id, Uuid::new_v4().to_string(), "__bind".into(),
        json!({ "config": full_config }), None,
    ).await {
        if let Some(merge) = result.get("mergeConfig").and_then(|v| v.as_object()) {
            save_config_secrets(&pool, &secrets, &secret_map, &json!(merge)).await?;
        }
    }

    Ok(Json(json!({ "message": "config saved" })))
}

pub async fn execute_action(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((integration_id, action_id)): Path<(String, String)>,
    Json(input): Json<JsonValue>,
) -> Result<Json<JsonValue>, ApiError> {
    let (pool, secrets) = routes::pool_and_secrets(&rt);
    let wm = routes::wm(&rt);

    let perm = format!("integration.{integration_id}.{action_id}");
    let (_, perms) = resolve_permissions(&pool, "core", identity.user_id).await?;
    if !perms.iter().any(|p| p == "*" || p == &perm) {
        return Err(ApiError::Forbidden(format!("permission denied: {perm}")));
    }

    let config = resolve_config(&pool, &secrets, &integration_id).await?;

    let key = format!("_iuc.{integration_id}.{}", identity.user_id);
    let user_credentials = match secrets.get(&pool, &integration_id, &key).await {
        Ok(Some(raw)) => serde_json::from_str(&raw).unwrap_or(JsonValue::Null),
        _ => JsonValue::Null,
    };

    let result = wm
        .rpc(
            &integration_id,
            Uuid::new_v4().to_string(),
            "__integration".into(),
            json!({ "action": action_id, "input": input, "config": config, "userCredentials": user_credentials, "userId": identity.user_id.to_string() }),
            None,
        )
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(result))
}

pub async fn webhook_ingress(
    State(rt): State<SharedRuntime>,
    Path(token): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<JsonValue>, ApiError> {
    let (pool, secrets) = routes::pool_and_secrets(&rt);
    let wm = routes::wm(&rt);

    let integration_id: String = sqlx::query_scalar(
        "SELECT id FROM rootcx_system.apps WHERE webhook_token = $1",
    )
    .bind(&token)
    .fetch_optional(&pool)
    .await?
    .ok_or_else(|| ApiError::NotFound("invalid webhook token".into()))?;

    let config = resolve_config(&pool, &secrets, &integration_id).await?;

    let hdr_map: JsonValue = headers
        .iter()
        .filter_map(|(k, v)| v.to_str().ok().map(|v| (k.as_str().to_string(), json!(v))))
        .collect::<serde_json::Map<String, JsonValue>>()
        .into();

    let payload = serde_json::from_slice(&body)
        .unwrap_or_else(|_| json!(String::from_utf8_lossy(&body).into_owned()));

    let raw_body = base64::engine::general_purpose::STANDARD.encode(&body);

    let result = wm
        .rpc(
            &integration_id,
            Uuid::new_v4().to_string(),
            "__webhook".into(),
            json!({ "headers": hdr_map, "body": payload, "rawBody": raw_body, "config": config }),
            None,
        )
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(result))
}

pub(super) async fn get_integration_manifest(pool: &sqlx::PgPool, integration_id: &str) -> Result<JsonValue, ApiError> {
    let row: Option<(JsonValue,)> = sqlx::query_as(
        "SELECT manifest FROM rootcx_system.apps WHERE id = $1 AND manifest->>'type' = 'integration'",
    )
    .bind(integration_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(m,)| m).unwrap_or(JsonValue::Null))
}

async fn save_config_secrets(
    pool: &sqlx::PgPool,
    secrets: &crate::secrets::SecretManager,
    secret_map: &[(String, String)],
    config: &JsonValue,
) -> Result<(), ApiError> {
    for (field, secret_key) in secret_map {
        if let Some(val) = config.get(field).and_then(|v| v.as_str()) {
            secrets.set(pool, PLATFORM_SCOPE, secret_key, val)
                .await
                .map_err(|e| ApiError::Internal(e.to_string()))?;
        }
    }
    Ok(())
}

async fn is_configured(
    pool: &sqlx::PgPool,
    secrets: &crate::secrets::SecretManager,
    manifest: &JsonValue,
    secret_map: &[(String, String)],
) -> Result<bool, ApiError> {
    if secret_map.is_empty() { return Ok(true); }
    let required: Vec<&str> = manifest.pointer("/configSchema/required")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    for (field, key) in secret_map {
        if required.contains(&field.as_str()) {
            if secrets.get(pool, PLATFORM_SCOPE, key).await
                .map_err(|e| ApiError::Internal(e.to_string()))?.is_none() {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

pub async fn resolve_config(
    pool: &sqlx::PgPool,
    secrets: &crate::secrets::SecretManager,
    integration_id: &str,
) -> Result<JsonValue, ApiError> {
    let manifest = get_integration_manifest(pool, integration_id).await?;
    fetch_config(pool, secrets, &platform_secret_map(&manifest)).await
}

async fn fetch_config(
    pool: &sqlx::PgPool,
    secrets: &crate::secrets::SecretManager,
    secret_map: &[(String, String)],
) -> Result<JsonValue, ApiError> {
    let mut config = serde_json::Map::new();
    for (field, secret_key) in secret_map {
        if let Some(val) = secrets.get(pool, PLATFORM_SCOPE, secret_key)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?
        {
            config.insert(field.clone(), JsonValue::String(val));
        }
    }
    if config.is_empty() { Ok(JsonValue::Null) } else { Ok(JsonValue::Object(config)) }
}
