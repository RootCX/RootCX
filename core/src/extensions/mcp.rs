use std::collections::HashMap;

use async_trait::async_trait;
use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value as JsonValue};
use sqlx::PgPool;
use tracing::{error, info};

use crate::RuntimeError;
use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::mcp::McpManager;
use crate::routes::SharedRuntime;
use crate::secrets::SecretManager;
use rootcx_types::McpServerConfig;

use super::RuntimeExtension;

pub struct McpExtension;

#[async_trait]
impl RuntimeExtension for McpExtension {
    fn name(&self) -> &str { "mcp" }

    async fn bootstrap(&self, pool: &PgPool) -> Result<(), RuntimeError> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS rootcx_system.mcp_servers (
                name       TEXT PRIMARY KEY,
                config     JSONB NOT NULL,
                status     TEXT NOT NULL DEFAULT 'stopped',
                created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
                updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
            )"
        ).execute(pool).await.map_err(RuntimeError::Schema)?;
        info!("mcp extension ready");
        Ok(())
    }

    fn routes(&self) -> Option<Router<SharedRuntime>> {
        Some(Router::new()
            .route("/api/v1/mcp-servers", get(list_servers).post(register_server))
            .route("/api/v1/mcp-servers/{name}", get(get_server).delete(remove_server))
            .route("/api/v1/mcp-servers/{name}/start", post(start_server))
            .route("/api/v1/mcp-servers/{name}/stop", post(stop_server)))
    }
}

async fn mcp_env(pool: &PgPool, secrets: &SecretManager) -> HashMap<String, String> {
    secrets.get_all_for_app(pool, "_platform").await
        .unwrap_or_default().into_iter().collect()
}

async fn set_status(pool: &PgPool, name: &str, status: &str) {
    let _ = sqlx::query("UPDATE rootcx_system.mcp_servers SET status = $2, updated_at = now() WHERE name = $1")
        .bind(name).bind(status).execute(pool).await;
}

fn server_json(name: &str, status: &str) -> JsonValue {
    json!({ "name": name, "status": status })
}

/// Sync tool registry to rbac_permissions after bulk MCP changes
async fn sync_tools(pool: &PgPool, mcp: &McpManager) {
    mcp.tool_registry().sync_to_db(pool).await;
}

pub async fn start_registered_servers(pool: &PgPool, secrets: &SecretManager, mcp: &McpManager) {
    let rows: Vec<(String, JsonValue)> = match sqlx::query_as(
        "SELECT name, config FROM rootcx_system.mcp_servers WHERE status = 'running'"
    ).fetch_all(pool).await {
        Ok(r) => r,
        Err(e) => { error!("load mcp_servers: {e}"); return; }
    };

    let env = mcp_env(pool, secrets).await;
    for (name, config_val) in rows {
        let Ok(config) = serde_json::from_value::<McpServerConfig>(config_val) else {
            error!(server = %name, "invalid mcp config in DB");
            continue;
        };
        match mcp.start_server(&config, &env).await {
            Ok(_) => {},
            Err(e) => {
                error!(server = %name, "auto-start failed: {e}");
                set_status(pool, &name, "error").await;
            }
        }
    }
    sync_tools(pool, mcp).await;
}

async fn list_servers(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
) -> Result<Json<Vec<JsonValue>>, ApiError> {
    let pool = crate::routes::pool(&rt);
    let rows: Vec<(String, JsonValue, String)> = sqlx::query_as(
        "SELECT name, config, status FROM rootcx_system.mcp_servers ORDER BY name"
    ).fetch_all(&pool).await?;

    Ok(Json(rows.into_iter().map(|(name, config, status)| {
        json!({ "name": name, "config": config, "status": status })
    }).collect()))
}

async fn get_server(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(name): Path<String>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = crate::routes::pool(&rt);
    let (name, config, status): (String, JsonValue, String) = sqlx::query_as(
        "SELECT name, config, status FROM rootcx_system.mcp_servers WHERE name = $1"
    ).bind(&name).fetch_optional(&pool).await?
        .ok_or_else(|| ApiError::NotFound(format!("MCP server '{name}'")))?;
    Ok(Json(json!({ "name": name, "config": config, "status": status })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegisterRequest {
    #[serde(flatten)]
    config: McpServerConfig,
    #[serde(default)]
    auto_start: bool,
}

async fn register_server(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Json(body): Json<RegisterRequest>,
) -> Result<Json<JsonValue>, ApiError> {
    let (pool, secrets) = crate::routes::pool_and_secrets(&rt);
    let name = &body.config.name;
    let config_val = serde_json::to_value(&body.config).map_err(|e| ApiError::Internal(e.to_string()))?;

    sqlx::query(
        "INSERT INTO rootcx_system.mcp_servers (name, config) VALUES ($1, $2)
         ON CONFLICT (name) DO UPDATE SET config = EXCLUDED.config, updated_at = now()"
    ).bind(name).bind(&config_val).execute(&pool).await?;

    if body.auto_start {
        let mcp = rt.mcp_manager().clone();
        let env = mcp_env(&pool, &secrets).await;
        match mcp.start_server(&body.config, &env).await {
            Ok(tools) => {
                set_status(&pool, name, "running").await;
                sync_tools(&pool, &mcp).await;
                return Ok(Json(json!({ "name": name, "status": "running", "tools": tools })));
            }
            Err(e) => {
                set_status(&pool, name, "error").await;
                return Err(ApiError::Internal(format!("start failed: {e}")));
            }
        }
    }

    Ok(Json(server_json(name, "stopped")))
}

async fn remove_server(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(name): Path<String>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = crate::routes::pool(&rt);
    let mcp = rt.mcp_manager().clone();
    mcp.stop_server(&name).await.map_err(|e| ApiError::Internal(e.to_string()))?;
    sync_tools(&pool, &mcp).await;
    sqlx::query("DELETE FROM rootcx_system.mcp_servers WHERE name = $1")
        .bind(&name).execute(&pool).await?;
    Ok(Json(json!({ "message": format!("MCP server '{name}' removed") })))
}

async fn start_server(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(name): Path<String>,
) -> Result<Json<JsonValue>, ApiError> {
    let (pool, secrets) = crate::routes::pool_and_secrets(&rt);
    let mcp = rt.mcp_manager().clone();

    if mcp.is_running(&name).await {
        return Ok(Json(server_json(&name, "running")));
    }

    let (config_val,): (JsonValue,) = sqlx::query_as(
        "SELECT config FROM rootcx_system.mcp_servers WHERE name = $1"
    ).bind(&name).fetch_optional(&pool).await?
        .ok_or_else(|| ApiError::NotFound(format!("MCP server '{name}'")))?;

    let config: McpServerConfig = serde_json::from_value(config_val)
        .map_err(|e| ApiError::Internal(format!("bad config: {e}")))?;
    let env = mcp_env(&pool, &secrets).await;
    let tools = mcp.start_server(&config, &env).await.map_err(|e| ApiError::Internal(e.to_string()))?;
    set_status(&pool, &name, "running").await;
    sync_tools(&pool, &mcp).await;

    Ok(Json(json!({ "name": name, "status": "running", "tools": tools })))
}

async fn stop_server(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(name): Path<String>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = crate::routes::pool(&rt);
    let mcp = rt.mcp_manager().clone();
    mcp.stop_server(&name).await.map_err(|e| ApiError::Internal(e.to_string()))?;
    set_status(&pool, &name, "stopped").await;
    sync_tools(&pool, &mcp).await;
    Ok(Json(server_json(&name, "stopped")))
}
