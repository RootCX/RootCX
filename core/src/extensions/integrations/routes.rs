use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use serde_json::{Value as JsonValue, json};
use uuid::Uuid;

use base64::Engine;

use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::extensions::rbac::policy::{require_admin, resolve_permissions};
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
    let pool = routes::pool(&rt);
    let mut items = query_installed_integrations(&pool).await?;
    let present_keys: std::collections::HashSet<String> =
        crate::secrets::SecretManager::list_keys(&pool, PLATFORM_SCOPE)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?
            .into_iter()
            .collect();
    for item in &mut items {
        let map = platform_secret_map(item);
        let configured = is_configured(item, &map, &present_keys);
        item.as_object_mut().unwrap().insert("configured".into(), json!(configured));
    }
    Ok(Json(items))
}

pub async fn save_platform_config(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(integration_id): Path<String>,
    Json(config): Json<JsonValue>,
) -> Result<Json<JsonValue>, ApiError> {
    let (pool, secrets) = routes::pool_and_secrets(&rt);
    require_admin(&pool, identity.user_id).await?;
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
    headers: HeaderMap,
    Json(input): Json<JsonValue>,
) -> Result<Json<JsonValue>, ApiError> {
    let (pool, secrets) = routes::pool_and_secrets(&rt);
    let wm = routes::wm(&rt);

    let perm = format!("integration:{integration_id}:{action_id}");
    let (_, perms) = resolve_permissions(&pool, identity.user_id).await?;
    if !crate::extensions::rbac::policy::has_permission(&perms, &perm) {
        return Err(ApiError::Forbidden(format!("permission denied: {perm}")));
    }

    // No impersonation: an integration action always runs as the authenticated
    // caller. Acting for a connected user is handled in-process by
    // execute_self_action (requester-scoped); owning automation is handled by
    // run_as on crons/jobs. There is no HTTP "act as anyone" header.
    let config = resolve_config(&pool, &secrets, &integration_id).await?;

    let target_user = identity.user_id.to_string();
    let caller_app = headers.get("x-app-id").and_then(|v| v.to_str().ok());
    let (user_credentials, effective_uid) = super::connections::resolve_credentials(
        &secrets, &pool, &integration_id, &target_user, caller_app,
    ).await;

    let caller_email = identity.email.clone();

    let caller = Some(crate::ipc::RpcCaller {
        user_id: target_user.clone(),
        email: caller_email,
        effective_perms: None,
    });

    let result = wm
        .rpc(
            &integration_id,
            Uuid::new_v4().to_string(),
            "__integration".into(),
            json!({ "action": action_id, "input": input, "config": config, "userCredentials": user_credentials, "userId": effective_uid }),
            caller,
        )
        .await
        .map_err(|e| {
            tracing::error!(integration = %integration_id, action = %action_id, "action failed: {e}");
            ApiError::Internal(e.to_string())
        })?;

    Ok(Json(result))
}

pub async fn connected_users(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(integration_id): Path<String>,
) -> Result<Json<Vec<String>>, ApiError> {
    let pool = routes::pool(&rt);
    require_admin(&pool, identity.user_id).await?;
    let users = super::connections::connected_users(&pool, &integration_id).await?;
    Ok(Json(users))
}

pub async fn webhook_ingress(
    State(rt): State<SharedRuntime>,
    Path(token): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<JsonValue>, ApiError> {
    let (pool, secrets) = routes::pool_and_secrets(&rt);
    let wm = routes::wm(&rt);

    let hdr_map: JsonValue = headers
        .iter()
        .filter_map(|(k, v)| v.to_str().ok().map(|v| (k.as_str().to_string(), json!(v))))
        .collect::<serde_json::Map<String, JsonValue>>()
        .into();

    let payload = serde_json::from_slice(&body)
        .unwrap_or_else(|_| json!(String::from_utf8_lossy(&body).into_owned()));

    let raw_body = base64::engine::general_purpose::STANDARD.encode(&body);

    if let Some(wh) = crate::webhooks::lookup_token(&pool, &token).await? {
        // Route by webhook method: "agent" dispatches to the app's agent;
        // anything else is a standard RPC call to the app worker.
        let is_agent_webhook = wh.method == "agent";

        if is_agent_webhook {
            let delegator = wh.created_by
                .ok_or_else(|| ApiError::Forbidden("webhook has no owner (created_by is NULL)".into()))?;
            let agent_uid = crate::extensions::agents::agent_user_id(&wh.app_id);
            if !crate::delegations::is_valid(&pool, delegator, agent_uid).await
                .map_err(|e| ApiError::Internal(e.to_string()))? {
                return Err(ApiError::Forbidden("no valid delegation for webhook agent".into()));
            }
            let message = format!("Webhook received: {}\n\nPayload:\n{payload}", wh.name);
            let llm = crate::routes::llm_models::fetch_default_llm(&pool).await
                .ok().flatten()
                .map(|(provider, model)| crate::ipc::LlmModelRef { provider, model });
            let invoke_payload = crate::ipc::AgentInvokePayload {
                invoke_id: Uuid::new_v4().to_string(),
                session_id: Uuid::new_v4().to_string(),
                message,
                history: vec![],
                is_sub_invoke: false,
                llm,
                invoker_user_id: Some(delegator),
                attachments: None,
            };
            let _ = wm.agent_invoke(&wh.app_id, invoke_payload, None).await
                .map_err(|e| ApiError::Internal(e.to_string()))?;
            return Ok(Json(json!({"status": "accepted"})));
        }

        // Standard webhook: dispatch to app RPC handler
        let caller = wh.created_by.map(|uid| crate::ipc::RpcCaller {
            user_id: uid.to_string(),
            email: String::new(),
            effective_perms: None,
        });
        let result = wm
            .rpc(
                &wh.app_id,
                Uuid::new_v4().to_string(),
                wh.method.clone(),
                json!({ "name": wh.name, "headers": hdr_map, "body": payload, "rawBody": raw_body }),
                caller,
            )
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;

        return Ok(Json(result));
    }

    // Legacy: integration webhooks (webhook_token on apps table)
    let integration_id: String = sqlx::query_scalar(
        "SELECT id FROM rootcx_system.apps WHERE webhook_token = $1",
    )
    .bind(&token)
    .fetch_optional(&pool)
    .await?
    .ok_or_else(|| ApiError::NotFound("invalid webhook token".into()))?;

    let config = resolve_config(&pool, &secrets, &integration_id).await?;

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
        let Some(val) = config.get(field).and_then(|v| v.as_str()) else { continue };
        secrets.set_or_delete(pool, PLATFORM_SCOPE, secret_key, val)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
    }
    Ok(())
}

fn is_configured(
    manifest: &JsonValue,
    secret_map: &[(String, String)],
    present_keys: &std::collections::HashSet<String>,
) -> bool {
    if secret_map.is_empty() { return true; }

    let field_to_key: std::collections::HashMap<&str, &str> = secret_map
        .iter()
        .map(|(f, k)| (f.as_str(), k.as_str()))
        .collect();
    let is_set = |key: &str| present_keys.contains(key);

    // JSON Schema `anyOf`: each branch declares its own `required` list.
    // At least one branch's required fields must all be present.
    // Used for integrations with multiple valid configuration modes
    // (e.g. Gmail managed vs self-hosted OAuth).
    if let Some(branches) = manifest
        .pointer("/configSchema/anyOf")
        .and_then(|v| v.as_array())
    {
        return branches.iter().any(|branch| {
            let Some(fields) = branch.pointer("/required").and_then(|v| v.as_array()) else { return false };
            !fields.is_empty() && fields.iter().all(|f| {
                f.as_str()
                    .and_then(|name| field_to_key.get(name))
                    .map(|key| is_set(key))
                    .unwrap_or(false)
            })
        });
    }

    // Fallback: required[] — every listed field must be present.
    let required: Vec<&str> = manifest.pointer("/configSchema/required")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();

    if required.is_empty() {
        // No declared requirements → integration has no secrets to gate on.
        // Only treat as configured if at least one secret is actually set.
        return secret_map.iter().any(|(_, key)| is_set(key));
    }

    required.iter().all(|field| {
        field_to_key.get(field).map(|key| is_set(key)).unwrap_or(false)
    })
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
