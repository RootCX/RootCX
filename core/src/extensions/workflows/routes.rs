use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use uuid::Uuid;

use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::governance::authority::{resolve_permissions, has_permission};
use crate::routes::{self, SharedRuntime};
use rootcx_types::WorkflowGraph;

use super::executor;

async fn authed(rt: &SharedRuntime, user_id: Uuid, app_id: &str) -> Result<(sqlx::PgPool, Vec<String>), ApiError> {
    let pool = routes::pool(rt);
    let (_, perms) = resolve_permissions(&pool, user_id).await?;
    if !has_permission(&perms, &format!("app:{app_id}:invoke")) {
        return Err(ApiError::Forbidden(format!("missing app:{app_id}:invoke")));
    }
    Ok((pool, perms))
}

// ── CRUD ─────────────────────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRow {
    id: Uuid,
    app_id: String,
    name: String,
    graph: JsonValue,
    enabled: bool,
    version: i32,
    created_at: String,
    updated_at: String,
}

pub async fn list_workflows(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
) -> Result<Json<Vec<WorkflowRow>>, ApiError> {
    let (pool, _) = authed(&rt, identity.user_id, &app_id).await?;

    let rows: Vec<(Uuid, String, String, JsonValue, bool, i32, String, String)> = sqlx::query_as(
        "SELECT id, app_id, name, graph, enabled, version, created_at::text, updated_at::text
         FROM rootcx_system.workflows WHERE app_id = $1 ORDER BY created_at DESC",
    ).bind(&app_id).fetch_all(&pool).await?;

    Ok(Json(rows.into_iter().map(|(id, app_id, name, graph, enabled, version, ca, ua)| WorkflowRow {
        id, app_id, name, graph, enabled, version, created_at: ca, updated_at: ua,
    }).collect()))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateWorkflow {
    name: String,
    #[serde(default)]
    graph: Option<WorkflowGraph>,
}

pub async fn create_workflow(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
    Json(body): Json<CreateWorkflow>,
) -> Result<(StatusCode, Json<JsonValue>), ApiError> {
    let (pool, _) = authed(&rt, identity.user_id, &app_id).await?;

    let graph = serde_json::to_value(body.graph.unwrap_or(WorkflowGraph { nodes: vec![], edges: vec![] }))
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO rootcx_system.workflows (app_id, name, graph, created_by)
         VALUES ($1, $2, $3, $4) RETURNING id",
    ).bind(&app_id).bind(&body.name).bind(&graph).bind(identity.user_id)
    .fetch_one(&pool).await.map_err(|e| {
        if e.to_string().contains("unique") || e.to_string().contains("duplicate") {
            ApiError::BadRequest(format!("workflow '{}' already exists", body.name))
        } else { ApiError::Internal(e.to_string()) }
    })?;

    Ok((StatusCode::CREATED, Json(json!({ "id": id }))))
}

pub async fn get_workflow(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((app_id, workflow_id)): Path<(String, Uuid)>,
) -> Result<Json<WorkflowRow>, ApiError> {
    let (pool, _) = authed(&rt, identity.user_id, &app_id).await?;

    let row: (Uuid, String, String, JsonValue, bool, i32, String, String) = sqlx::query_as(
        "SELECT id, app_id, name, graph, enabled, version, created_at::text, updated_at::text
         FROM rootcx_system.workflows WHERE id = $1 AND app_id = $2",
    ).bind(workflow_id).bind(&app_id).fetch_optional(&pool).await?
    .ok_or_else(|| ApiError::NotFound("workflow not found".into()))?;

    let (id, app_id, name, graph, enabled, version, ca, ua) = row;
    Ok(Json(WorkflowRow { id, app_id, name, graph, enabled, version, created_at: ca, updated_at: ua }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateWorkflow {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    graph: Option<WorkflowGraph>,
    #[serde(default)]
    enabled: Option<bool>,
}

pub async fn update_workflow(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((app_id, workflow_id)): Path<(String, Uuid)>,
    Json(body): Json<UpdateWorkflow>,
) -> Result<Json<JsonValue>, ApiError> {
    let (pool, _) = authed(&rt, identity.user_id, &app_id).await?;

    let graph_json = body.graph.map(|g| serde_json::to_value(g).unwrap_or_default());

    let r = sqlx::query(
        "UPDATE rootcx_system.workflows SET
            name = COALESCE($3, name),
            graph = COALESCE($4, graph),
            enabled = COALESCE($5, enabled),
            version = version + CASE WHEN $4 IS NOT NULL THEN 1 ELSE 0 END,
            updated_at = now()
         WHERE id = $1 AND app_id = $2",
    ).bind(workflow_id).bind(&app_id).bind(&body.name).bind(&graph_json).bind(body.enabled)
    .execute(&pool).await?;

    if r.rows_affected() == 0 { return Err(ApiError::NotFound("workflow not found".into())); }
    Ok(Json(json!({ "updated": true })))
}

pub async fn delete_workflow(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((app_id, workflow_id)): Path<(String, Uuid)>,
) -> Result<Json<JsonValue>, ApiError> {
    let (pool, _) = authed(&rt, identity.user_id, &app_id).await?;

    let r = sqlx::query("DELETE FROM rootcx_system.workflows WHERE id = $1 AND app_id = $2")
        .bind(workflow_id).bind(&app_id).execute(&pool).await?;
    if r.rows_affected() == 0 { return Err(ApiError::NotFound("workflow not found".into())); }
    Ok(Json(json!({ "deleted": true })))
}

// ── Node palette ─────────────────────────────────────────────────────

pub async fn list_nodes(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
) -> Result<Json<JsonValue>, ApiError> {
    let (pool, perms) = authed(&rt, identity.user_id, &app_id).await?;

    let contract: JsonValue = sqlx::query_scalar(
        "SELECT COALESCE(manifest->'dataContract', '[]'::jsonb) FROM rootcx_system.apps WHERE id = $1",
    ).bind(&app_id).fetch_optional(&pool).await?.unwrap_or_default();

    let descriptors = rt.tool_registry().descriptors_for_permissions(&perms, &contract);
    Ok(Json(serde_json::to_value(descriptors).unwrap_or_default()))
}

// ── Run (synchronous, minimal v1) ───────────────────────────────────

pub async fn run_workflow(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((app_id, workflow_id)): Path<(String, Uuid)>,
) -> Result<Json<JsonValue>, ApiError> {
    let (pool, perms) = authed(&rt, identity.user_id, &app_id).await?;

    let (graph_json,): (JsonValue,) = sqlx::query_as(
        "SELECT graph FROM rootcx_system.workflows WHERE id = $1 AND app_id = $2 AND enabled = true",
    ).bind(workflow_id).bind(&app_id).fetch_optional(&pool).await?
    .ok_or_else(|| ApiError::NotFound("workflow not found or not enabled".into()))?;

    let graph: WorkflowGraph = serde_json::from_value(graph_json)
        .map_err(|e| ApiError::Internal(format!("invalid graph: {e}")))?;

    let exec_id: Uuid = sqlx::query_scalar(
        "INSERT INTO rootcx_system.workflow_executions (workflow_id, app_id, status, run_as_user_id, started_at)
         VALUES ($1, $2, 'running', $3, now()) RETURNING id",
    ).bind(workflow_id).bind(&app_id).bind(identity.user_id)
    .fetch_one(&pool).await?;

    let results = executor::execute_dag(&rt, &pool, &app_id, identity.user_id, &perms, &graph, exec_id).await;

    let all_ok = results.iter().all(|r| r.status == rootcx_types::WorkflowNodeRunStatus::Succeeded);
    let final_status = if all_ok { rootcx_types::WorkflowExecutionStatus::Succeeded } else { rootcx_types::WorkflowExecutionStatus::Failed };
    let error_msg = results.iter().find_map(|r| {
        if r.status == rootcx_types::WorkflowNodeRunStatus::Failed { r.error.clone() } else { None }
    });

    sqlx::query(
        "UPDATE rootcx_system.workflow_executions SET status = $2, error = $3, finished_at = now() WHERE id = $1",
    ).bind(exec_id).bind(final_status.as_str()).bind(&error_msg)
    .execute(&pool).await?;

    Ok(Json(json!({
        "executionId": exec_id,
        "status": final_status.as_str(),
        "nodeRuns": results.iter().map(|r| json!({
            "nodeId": r.node_id, "status": r.status.as_str(), "error": r.error,
        })).collect::<Vec<_>>(),
    })))
}

// ── Executions list ──────────────────────────────────────────────────

pub async fn list_executions(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((app_id, workflow_id)): Path<(String, Uuid)>,
) -> Result<Json<JsonValue>, ApiError> {
    let (pool, _) = authed(&rt, identity.user_id, &app_id).await?;

    let rows: Vec<(Uuid, String, Option<String>, String, Option<String>)> = sqlx::query_as(
        "SELECT id, status, error, created_at::text, finished_at::text
         FROM rootcx_system.workflow_executions WHERE workflow_id = $1 ORDER BY created_at DESC LIMIT 50",
    ).bind(workflow_id).fetch_all(&pool).await?;

    Ok(Json(json!(rows.into_iter().map(|(id, status, error, ca, fa)| json!({
        "id": id, "status": status, "error": error, "createdAt": ca, "finishedAt": fa,
    })).collect::<Vec<_>>())))
}
