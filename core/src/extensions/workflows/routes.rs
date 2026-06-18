use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{Value as JsonValue, json};
use uuid::Uuid;

use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::governance::authority::resolve_permissions;
use crate::routes::{self, SharedRuntime};
use rootcx_types::WorkflowGraph;

use super::executor;

fn wf_app_id(workflow_id: Uuid) -> String {
    format!("wf-{workflow_id}")
}

// ── CRUD ─────────────────────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRow {
    id: Uuid,
    name: String,
    enabled: bool,
    version: i32,
    created_at: String,
    updated_at: String,
}

pub async fn list_workflows(
    identity: Identity,
    State(rt): State<SharedRuntime>,
) -> Result<Json<Vec<WorkflowRow>>, ApiError> {
    let pool = routes::pool(&rt);

    let rows: Vec<(Uuid, String, bool, i32, String, String)> = sqlx::query_as(
        "SELECT id, name, enabled, version, created_at::text, updated_at::text
         FROM rootcx_system.workflows WHERE created_by = $1 OR EXISTS (
            SELECT 1 FROM rootcx_system.rbac_assignments ra
            JOIN rootcx_system.rbac_roles rr ON rr.name = ra.role
            WHERE ra.user_id = $1 AND ('admin' = ANY(rr.permissions) OR '*' = ANY(rr.permissions))
         )
         ORDER BY created_at DESC",
    ).bind(identity.user_id).fetch_all(&pool).await?;

    Ok(Json(rows.into_iter().map(|(id, name, enabled, version, ca, ua)| WorkflowRow {
        id, name, enabled, version, created_at: ca, updated_at: ua,
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
    Json(body): Json<CreateWorkflow>,
) -> Result<(StatusCode, Json<JsonValue>), ApiError> {
    let pool = routes::pool(&rt);

    let graph = serde_json::to_value(body.graph.unwrap_or(WorkflowGraph { nodes: vec![], edges: vec![] }))
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let wf_id = Uuid::new_v4();
    let app_id = wf_app_id(wf_id);
    let role_name = format!("app:{app_id}:owner");

    // Provision backing app + owner role + workflow row atomically: a failed
    // insert (e.g. duplicate name) must not leave an orphan app or role behind.
    let mut tx = pool.begin().await?;

    sqlx::query(
        "INSERT INTO rootcx_system.apps (id, name, version, status, manifest)
         VALUES ($1, $2, '0.0.0', 'system', $3)",
    ).bind(&app_id).bind(format!("Workflow: {}", body.name))
    .bind(json!({"appId": &app_id, "name": &body.name, "type": "workflow"}))
    .execute(&mut *tx).await.map_err(|e| ApiError::Internal(e.to_string()))?;

    sqlx::query(
        "INSERT INTO rootcx_system.rbac_roles (name, description, permissions)
         VALUES ($1, $2, $3) ON CONFLICT (name) DO NOTHING",
    ).bind(&role_name).bind(format!("Owner of workflow {}", body.name))
    .bind(&vec![format!("app:{app_id}:*"), "tool:*".to_string()])
    .execute(&mut *tx).await?;

    sqlx::query(
        "INSERT INTO rootcx_system.rbac_assignments (user_id, role) VALUES ($1, $2) ON CONFLICT DO NOTHING",
    ).bind(identity.user_id).bind(&role_name).execute(&mut *tx).await?;

    sqlx::query(
        "INSERT INTO rootcx_system.workflows (id, app_id, name, graph, created_by)
         VALUES ($1, $2, $3, $4, $5)",
    ).bind(wf_id).bind(&app_id).bind(&body.name).bind(&graph).bind(identity.user_id)
    .execute(&mut *tx).await.map_err(|e| {
        if e.to_string().contains("unique") || e.to_string().contains("duplicate") {
            ApiError::BadRequest(format!("workflow '{}' already exists", body.name))
        } else { ApiError::Internal(e.to_string()) }
    })?;

    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(json!({ "id": wf_id }))))
}

pub async fn get_workflow(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(workflow_id): Path<Uuid>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt);

    let row: Option<(Uuid, String, String, JsonValue, bool, i32, String, String)> = sqlx::query_as(
        "SELECT id, app_id, name, graph, enabled, version, created_at::text, updated_at::text
         FROM rootcx_system.workflows WHERE id = $1",
    ).bind(workflow_id).fetch_optional(&pool).await?;

    let (id, _, name, graph, enabled, version, ca, ua) = row
        .ok_or_else(|| ApiError::NotFound("workflow not found".into()))?;

    Ok(Json(json!({
        "id": id, "name": name, "graph": graph, "enabled": enabled,
        "version": version, "createdAt": ca, "updatedAt": ua,
    })))
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
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(workflow_id): Path<Uuid>,
    Json(body): Json<UpdateWorkflow>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt);
    let graph_json = body.graph.map(|g| serde_json::to_value(g).unwrap_or_default());

    let r = sqlx::query(
        "UPDATE rootcx_system.workflows SET
            name = COALESCE($2, name),
            graph = COALESCE($3, graph),
            enabled = COALESCE($4, enabled),
            version = version + CASE WHEN $3 IS NOT NULL THEN 1 ELSE 0 END,
            updated_at = now()
         WHERE id = $1",
    ).bind(workflow_id).bind(&body.name).bind(&graph_json).bind(body.enabled)
    .execute(&pool).await?;

    if r.rows_affected() == 0 { return Err(ApiError::NotFound("workflow not found".into())); }
    Ok(Json(json!({ "updated": true })))
}

pub async fn delete_workflow(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(workflow_id): Path<Uuid>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt);

    // Get the backing app_id to clean up
    let app_id: Option<String> = sqlx::query_scalar(
        "SELECT app_id FROM rootcx_system.workflows WHERE id = $1",
    ).bind(workflow_id).fetch_optional(&pool).await?;

    let r = sqlx::query("DELETE FROM rootcx_system.workflows WHERE id = $1")
        .bind(workflow_id).execute(&pool).await?;
    if r.rows_affected() == 0 { return Err(ApiError::NotFound("workflow not found".into())); }

    // Clean up backing app
    if let Some(aid) = app_id {
        sqlx::query("DELETE FROM rootcx_system.apps WHERE id = $1").bind(&aid).execute(&pool).await.ok();
    }

    Ok(Json(json!({ "deleted": true })))
}

// ── Node palette ─────────────────────────────────────────────────────

/// Palette = capability tools (call_action, invoke_agent, ...) + `data`: the
/// CRUD surface expanded per visible (app, entity). Each data entry becomes a
/// `query_data`/`mutate_data` preset on the client; both generic tools stay in
/// `tools` so the config panel can resolve their schema. Governance is unchanged:
/// dispatch gates `tool:{query,mutate}_data`, RLS enforces per-app/per-row.
pub async fn list_nodes(
    identity: Identity,
    State(rt): State<SharedRuntime>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt);
    let (_, perms) = resolve_permissions(&pool, identity.user_id).await?;
    let tools = rt.tool_registry().descriptors_for_permissions(&perms, &json!([]));

    let mut data = Vec::new();
    use crate::governance::authority::has_permission;
    if has_permission(&perms, "tool:query_data") || has_permission(&perms, "tool:mutate_data") {
        let apps: Vec<(String, String, JsonValue)> = sqlx::query_as(
            "SELECT id, name, COALESCE(manifest->'dataContract', '[]'::jsonb)
             FROM rootcx_system.apps WHERE status = 'installed' ORDER BY name",
        ).fetch_all(&pool).await?;
        for (id, name, dc) in apps {
            let Some(entities) = dc.as_array() else { continue };
            for e in entities {
                if let Some(entity) = e.get("entityName").and_then(|v| v.as_str()) {
                    data.push(json!({ "app": id, "appName": name, "entity": entity }));
                }
            }
        }
    }

    Ok(Json(json!({ "tools": tools, "data": data })))
}

// ── Run ──────────────────────────────────────────────────────────────

pub async fn run_workflow(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(workflow_id): Path<Uuid>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt);
    let (_, perms) = resolve_permissions(&pool, identity.user_id).await?;

    let app_id: String = sqlx::query_scalar(
        "SELECT app_id FROM rootcx_system.workflows WHERE id = $1 AND enabled = true",
    ).bind(workflow_id).fetch_optional(&pool).await?
    .ok_or_else(|| ApiError::NotFound("workflow not found or not enabled".into()))?;

    let (exec_id, results) = executor::run_workflow(
        rt.tool_registry(), &pool, &app_id, workflow_id, identity.user_id, &perms, None,
    ).await.map_err(|e| ApiError::Internal(e))?;

    Ok(Json(json!({
        "executionId": exec_id,
        "status": if results.iter().all(|r| r.status == rootcx_types::WorkflowNodeRunStatus::Succeeded) { "succeeded" } else { "failed" },
        "nodeRuns": results.iter().map(|r| json!({
            "nodeId": r.node_id, "status": r.status.as_str(), "error": r.error,
        })).collect::<Vec<_>>(),
    })))
}

// ── Executions list ──────────────────────────────────────────────────

pub async fn list_executions(
    _identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(workflow_id): Path<Uuid>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt);

    let rows: Vec<(Uuid, String, Option<String>, String, Option<String>)> = sqlx::query_as(
        "SELECT id, status, error, created_at::text, finished_at::text
         FROM rootcx_system.workflow_executions WHERE workflow_id = $1 ORDER BY created_at DESC LIMIT 50",
    ).bind(workflow_id).fetch_all(&pool).await?;

    Ok(Json(json!(rows.into_iter().map(|(id, status, error, ca, fa)| json!({
        "id": id, "status": status, "error": error, "createdAt": ca, "finishedAt": fa,
    })).collect::<Vec<_>>())))
}
