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

use super::events::{LiveEvent, WorkflowEvent};

fn wf_app_id(workflow_id: Uuid) -> String {
    format!("wf-{workflow_id}")
}

/// Per-workflow authorization: the caller must own it (`created_by`) or be an
/// admin — mirrors the `list_workflows` visibility predicate. Returns NotFound
/// (not Forbidden) so a probe can't tell "exists, not yours" from "absent". Every
/// workflow/execution-scoped read or mutation goes through this; executions inherit
/// their workflow's authority (no per-user RLS on the system-schema tables).
async fn authorize_workflow(pool: &sqlx::PgPool, user_id: Uuid, workflow_id: Uuid) -> Result<(), ApiError> {
    let ok: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM rootcx_system.workflows w WHERE w.id = $1 AND (
            w.created_by = $2 OR EXISTS (
                SELECT 1 FROM rootcx_system.rbac_assignments ra
                JOIN rootcx_system.rbac_roles rr ON rr.name = ra.role
                WHERE ra.user_id = $2 AND ('admin' = ANY(rr.permissions) OR '*' = ANY(rr.permissions))
            )))",
    ).bind(workflow_id).bind(user_id).fetch_one(pool).await?;
    if ok { Ok(()) } else { Err(ApiError::NotFound("workflow not found".into())) }
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
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(workflow_id): Path<Uuid>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt);
    authorize_workflow(&pool, identity.user_id, workflow_id).await?;

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
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(workflow_id): Path<Uuid>,
    Json(body): Json<UpdateWorkflow>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt);
    authorize_workflow(&pool, identity.user_id, workflow_id).await?;
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

    // Auto-register/unregister triggers when enabled state changes.
    if body.enabled.is_some() {
        sync_record_change_hooks(&pool, workflow_id, identity.user_id).await;
        sync_schedule_cron(&pool, workflow_id, identity.user_id).await;
    }

    Ok(Json(json!({ "updated": true })))
}

pub async fn delete_workflow(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(workflow_id): Path<Uuid>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt);
    authorize_workflow(&pool, identity.user_id, workflow_id).await?;

    // Get the backing app_id to clean up
    let app_id: Option<String> = sqlx::query_scalar(
        "SELECT app_id FROM rootcx_system.workflows WHERE id = $1",
    ).bind(workflow_id).fetch_optional(&pool).await?;

    let r = sqlx::query("DELETE FROM rootcx_system.workflows WHERE id = $1")
        .bind(workflow_id).execute(&pool).await?;
    if r.rows_affected() == 0 { return Err(ApiError::NotFound("workflow not found".into())); }

    // Remove hooks owned by this workflow before deleting the backing app.
    sqlx::query(
        "DELETE FROM rootcx_system.entity_hooks WHERE action_type = 'workflow' AND action_config->>'workflow_id' = $1",
    ).bind(workflow_id.to_string()).execute(&pool).await.ok();

    // Clean up backing app
    if let Some(aid) = app_id {
        sqlx::query("DELETE FROM rootcx_system.apps WHERE id = $1").bind(&aid).execute(&pool).await.ok();
    }

    Ok(Json(json!({ "deleted": true })))
}

/// Auto-register/unregister entity hooks for record-change triggers. Called on
/// enable/disable and graph save. Idempotent: drops stale hooks, creates missing.
async fn sync_record_change_hooks(pool: &sqlx::PgPool, workflow_id: Uuid, user_id: Uuid) {
    let row: Option<(bool, JsonValue)> = sqlx::query_as(
        "SELECT enabled, graph FROM rootcx_system.workflows WHERE id = $1",
    ).bind(workflow_id).fetch_optional(pool).await.ok().flatten();

    let Some((enabled, graph_json)) = row else { return };

    // Remove all hooks for this workflow first (idempotent reconciliation).
    sqlx::query(
        "DELETE FROM rootcx_system.entity_hooks WHERE action_type = 'workflow' AND action_config->>'workflow_id' = $1",
    ).bind(workflow_id.to_string()).execute(pool).await.ok();

    if !enabled { return; }

    // Find the recordChange trigger node and its configured params.
    let graph: rootcx_types::WorkflowGraph = match serde_json::from_value(graph_json) {
        Ok(g) => g,
        Err(_) => return,
    };
    let trigger = graph.nodes.iter().find(|n| matches!(
        &n.kind, rootcx_types::WorkflowNodeKind::Trigger { trigger: rootcx_types::TriggerKind::RecordChange }
    ));
    let Some(t) = trigger else { return };

    let app = t.params.get("app").and_then(|v| v.as_str());
    let entity = t.params.get("entity").and_then(|v| v.as_str());
    let operation = t.params.get("operation").and_then(|v| v.as_str());
    let (Some(app), Some(entity), Some(operation)) = (app, entity, operation) else { return };

    sqlx::query(
        "INSERT INTO rootcx_system.entity_hooks (app_id, entity, operation, action_type, action_config, active, created_by)
         VALUES ($1, $2, $3, 'workflow', $4, true, $5)",
    ).bind(app).bind(entity).bind(operation)
    .bind(json!({ "workflow_id": workflow_id }))
    .bind(user_id)
    .execute(pool).await.ok();
}

/// Auto-register/unregister a pg_cron schedule for schedule-triggered workflows.
async fn sync_schedule_cron(pool: &sqlx::PgPool, workflow_id: Uuid, user_id: Uuid) {
    let row: Option<(String, bool, JsonValue)> = sqlx::query_as(
        "SELECT app_id, enabled, graph FROM rootcx_system.workflows WHERE id = $1",
    ).bind(workflow_id).fetch_optional(pool).await.ok().flatten();

    let Some((app_id, enabled, graph_json)) = row else { return };

    // Remove any existing cron for this workflow (idempotent reconciliation).
    let existing: Vec<uuid::Uuid> = sqlx::query_scalar(
        "SELECT id FROM rootcx_system.cron_schedules WHERE app_id = $1 AND payload->>'workflow_id' = $2",
    ).bind(&app_id).bind(workflow_id.to_string()).fetch_all(pool).await.unwrap_or_default();
    for cron_id in existing {
        crate::crons::delete(pool, &app_id, cron_id).await.ok();
    }

    if !enabled { return; }

    let graph: rootcx_types::WorkflowGraph = match serde_json::from_value(graph_json) {
        Ok(g) => g,
        Err(_) => return,
    };
    let trigger = graph.nodes.iter().find(|n| matches!(
        &n.kind, rootcx_types::WorkflowNodeKind::Trigger { trigger: rootcx_types::TriggerKind::Schedule }
    ));
    let Some(t) = trigger else { return };
    let Some(schedule) = t.params.get("schedule").and_then(|v| v.as_str()) else { return };

    crate::crons::create(pool, &app_id, crate::crons::CreateCron {
        name: format!("wf-{workflow_id}-schedule"),
        schedule: schedule.to_string(),
        timezone: None,
        payload: json!({ "workflow_id": workflow_id }),
        overlap_policy: "skip".to_string(),
        created_by: Some(user_id),
    }).await.ok();
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

/// Manual run: snapshot an execution (run-as caller) and enqueue it, returning the
/// id immediately. Like every other run it now goes through pgmq under a lease;
/// the editor streams progress via `stream_execution`.
pub async fn run_workflow(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(workflow_id): Path<Uuid>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt);
    authorize_workflow(&pool, identity.user_id, workflow_id).await?;

    let app_id: String = sqlx::query_scalar(
        "SELECT app_id FROM rootcx_system.workflows WHERE id = $1 AND enabled = true",
    ).bind(workflow_id).fetch_optional(&pool).await?
    .ok_or_else(|| ApiError::NotFound("workflow not found or not enabled".into()))?;

    let exec_id = super::runner::create_execution(&pool, workflow_id, &app_id, identity.user_id, None, None)
        .await.map_err(ApiError::Internal)?;

    let payload = json!({ "action_type": "workflow", "manual": true, "execution_id": exec_id });
    let msg_id = crate::jobs::enqueue(&pool, &app_id, payload, Some(identity.user_id))
        .await.map_err(|e| ApiError::Internal(e.to_string()))?;
    // Bind the lease so a redelivery resumes this very execution.
    sqlx::query("UPDATE rootcx_system.workflow_executions SET lease_msg_id = $2 WHERE id = $1")
        .bind(exec_id).bind(msg_id).execute(&pool).await?;
    rt.wake_scheduler();

    Ok(Json(json!({ "executionId": exec_id, "status": "queued" })))
}

/// SSE stream of an execution's progress. Replays persisted node_runs (durable
/// source of truth, so a late or reconnecting client misses nothing), then
/// forwards live per-node events until the terminal `done`.
pub async fn stream_execution(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((workflow_id, exec_id)): Path<(Uuid, Uuid)>,
) -> Result<axum::response::Sse<impl futures::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>>, ApiError> {
    use axum::response::sse::{Event, KeepAlive, Sse};
    use futures::StreamExt;

    let pool = routes::pool(&rt);
    authorize_workflow(&pool, identity.user_id, workflow_id).await?;
    // Subscribe before reading state so no event fires in the gap.
    let rx = rt.workflow_events().subscribe(exec_id);

    // Scope by workflow_id so an owned workflow can't be used to read another's run.
    let status: String = sqlx::query_scalar(
        "SELECT status FROM rootcx_system.workflow_executions WHERE id = $1 AND workflow_id = $2",
    ).bind(exec_id).bind(workflow_id).fetch_optional(&pool).await?
    .ok_or_else(|| ApiError::NotFound("execution not found".into()))?;

    let rows: Vec<(String, String, Option<JsonValue>, Option<String>)> = sqlx::query_as(
        "SELECT node_id, status, output, error FROM rootcx_system.workflow_node_runs
         WHERE execution_id = $1 ORDER BY started_at",
    ).bind(exec_id).fetch_all(&pool).await?;

    let mut replay: Vec<Event> = rows.iter()
        .map(|(nid, st, out, err)| node_sse(nid, st, out.clone(), err.clone()))
        .collect();

    let terminal = matches!(status.as_str(), "succeeded" | "failed" | "canceled");
    let live = if terminal {
        // Already finished before the client attached: close it out from the row,
        // and drop the channel our `subscribe` may have re-created so the per-exec
        // map can't accumulate orphans from reconnects on finished runs.
        replay.push(done_sse(&status, None));
        rt.workflow_events().close(exec_id);
        futures::stream::empty().left_stream()
    } else {
        futures::stream::unfold(rx, |mut rx| async move {
            loop {
                match rx.recv().await {
                    Ok(WorkflowEvent::Node { node_id, status, output, error }) =>
                        return Some((Ok(node_sse(&node_id, &status, Some(output), error)), rx)),
                    Ok(WorkflowEvent::Done { status, error }) =>
                        return Some((Ok(done_sse(&status, error)), rx)),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => return None, // sender dropped after Done → end
                }
            }
        }).right_stream()
    };

    let stream = futures::stream::iter(replay.into_iter().map(Ok)).chain(live);
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

fn node_sse(node_id: &str, status: &str, output: Option<JsonValue>, error: Option<String>) -> axum::response::sse::Event {
    axum::response::sse::Event::default().event("node").data(
        json!({ "nodeId": node_id, "status": status, "output": output, "error": error }).to_string()
    )
}

fn done_sse(status: &str, error: Option<String>) -> axum::response::sse::Event {
    axum::response::sse::Event::default().event("done").data(
        json!({ "status": status, "error": error }).to_string()
    )
}

/// SSE stream of ALL runs for a workflow (the editor subscribes on page load).
/// Emits per-node and terminal events for every execution as it happens — the
/// nodes light up in real time regardless of who/what triggered the run.
pub async fn stream_workflow_live(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(workflow_id): Path<Uuid>,
) -> Result<axum::response::Sse<impl futures::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>>, ApiError> {
    use axum::response::sse::{Event, KeepAlive, Sse};

    let pool = routes::pool(&rt);
    authorize_workflow(&pool, identity.user_id, workflow_id).await?;

    let rx = rt.workflow_events().subscribe_workflow(workflow_id);

    let stream = futures::stream::unfold(rx, |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    let data = serde_json::to_string(&ev).unwrap_or_default();
                    let event_type = match &ev.event {
                        WorkflowEvent::Node { .. } => "node",
                        WorkflowEvent::Done { .. } => "done",
                    };
                    let sse = Event::default().event(event_type).data(data);
                    return Some((Ok(sse), rx));
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => return None,
            }
        }
    });

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

// ── Executions list ──────────────────────────────────────────────────

pub async fn list_executions(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(workflow_id): Path<Uuid>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt);
    authorize_workflow(&pool, identity.user_id, workflow_id).await?;

    let rows: Vec<(Uuid, String, Option<String>, String, Option<String>)> = sqlx::query_as(
        "SELECT id, status, error, created_at::text, finished_at::text
         FROM rootcx_system.workflow_executions WHERE workflow_id = $1 ORDER BY created_at DESC LIMIT 50",
    ).bind(workflow_id).fetch_all(&pool).await?;

    Ok(Json(json!(rows.into_iter().map(|(id, status, error, ca, fa)| json!({
        "id": id, "status": status, "error": error, "createdAt": ca, "finishedAt": fa,
    })).collect::<Vec<_>>())))
}

// ── Webhook trigger ─────────────────────────────────────────────────

/// Public endpoint (no auth). An HTTP POST triggers the workflow run-as-owner via
/// fire_gate, with the request body as trigger_data. The workflow must be enabled
/// and have a `webhook` trigger node.
pub async fn webhook_trigger(
    State(rt): State<SharedRuntime>,
    Path(workflow_id): Path<Uuid>,
    body: axum::body::Bytes,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt);

    let (app_id, owner): (String, Option<uuid::Uuid>) = sqlx::query_as(
        "SELECT app_id, created_by FROM rootcx_system.workflows WHERE id = $1 AND enabled = true",
    ).bind(workflow_id).fetch_optional(&pool).await?
    .ok_or_else(|| ApiError::NotFound("workflow not found or not enabled".into()))?;

    let trigger_data: JsonValue = serde_json::from_slice(&body).unwrap_or_else(|_| {
        json!({ "raw": String::from_utf8_lossy(&body).into_owned() })
    });

    // Webhook payload goes directly into trigger_data (not wrapped in a hook
    // envelope), so $json in the workflow = the POST body as-is.
    let payload = json!({
        "action_type": "workflow",
        "_hook": true,
        "action_config": { "workflow_id": workflow_id },
        "record": trigger_data,
    });
    crate::jobs::enqueue(&pool, &app_id, payload, owner)
        .await.map_err(|e| ApiError::Internal(e.to_string()))?;
    rt.wake_scheduler();

    Ok(Json(json!({ "status": "accepted" })))
}
