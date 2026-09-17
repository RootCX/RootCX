use axum::{Json, Router, extract::{Path, State}, routing::{get, post}};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

use crate::{api_error::ApiError, auth::identity::Identity, routes::SharedRuntime};
use super::{CURRENT_APPROVAL, artifacts};

pub(crate) fn routes() -> Router<SharedRuntime> {
    Router::new()
        .route("/api/v1/apps/{app}/action-approvals", get(inspect))
        .route("/api/v1/apps/{app}/action-approvals/{action}", post(approve).delete(revoke))
}

async fn snapshot(pool: &sqlx::PgPool, app: &str) -> Result<Value, ApiError> {
    let row = sqlx::query(
        "SELECT a.manifest,i.id AS installation_id,r.revision,r.digest
         FROM rootcx_system.apps a
         JOIN rootcx_system.app_installations i ON i.app_id=a.id AND i.active
         LEFT JOIN rootcx_system.backend_releases r ON r.app_id=a.id
         WHERE a.id=$1 AND a.status IN ('installed','system')",
    ).bind(app).fetch_optional(pool).await?
        .ok_or_else(|| ApiError::NotFound(format!("app '{app}' is not installed")))?;
    let manifest: Value = row.get("manifest");
    let active = sqlx::query(&format!("{CURRENT_APPROVAL} AND g.app_id=$1"))
        .bind(app).fetch_all(pool).await?;
    let actions: Vec<Value> = manifest["actions"].as_array().into_iter().flatten()
        .filter(|action| !action["authority"].is_null())
        .map(|action| {
            let approval = active.iter().find(|r| r.get::<String, _>("action_id") == action["id"]);
            json!({
                "id": action["id"], "authority": action["authority"],
                "approvalId": approval.map(|r| r.get::<Uuid,_>("id")),
                "status": if approval.is_some() {"approved"} else {"pending"}
            })
        }).collect();
    Ok(json!({
        "revision": row.get::<Option<Uuid>,_>("revision"),
        "backendDigest": row.get::<Option<String>,_>("digest"),
        "installationId": row.get::<Uuid,_>("installation_id"), "actions": actions,
    }))
}

async fn inspect(
    identity: Identity, State(rt): State<SharedRuntime>, Path(app): Path<String>,
) -> Result<Json<Value>, ApiError> {
    crate::governance::authority::require_perm(rt.pool(), identity.user_id, "admin:apps.deploy").await?;
    let _lifecycle = crate::governance::cross_app::lock_app_lifecycle(rt.pool(), &app).await?;
    Ok(Json(snapshot(rt.pool(), &app).await?))
}

#[derive(Deserialize)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
struct ApprovalRequest {
    revision: Uuid,
    backend_digest: String,
    installation_id: Uuid,
}

async fn approve(
    identity: Identity, State(rt): State<SharedRuntime>, Path((app, action)): Path<(String, String)>,
    Json(request): Json<ApprovalRequest>,
) -> Result<Json<Value>, ApiError> {
    crate::governance::authority::require_admin(rt.pool(), identity.user_id).await?;
    let _lifecycle = crate::governance::cross_app::lock_app_lifecycle(rt.pool(), &app).await?;
    let preview = snapshot(rt.pool(), &app).await?;
    if preview["revision"] != json!(request.revision)
        || preview["backendDigest"] != request.backend_digest
        || preview["installationId"] != json!(request.installation_id)
    {
        return Err(ApiError::Conflict("release changed since review; inspect action approvals again".into()));
    }
    let declaration = preview["actions"].as_array().and_then(|actions| actions.iter().find(|a| a["id"] == action))
        .ok_or_else(|| ApiError::BadRequest(format!("'{action}' does not declare action authority")))?;
    crate::worker::verify_approved_prelude(&rt.data_dir().join("apps/.prelude.js"))
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    artifacts::seal(rt.data_dir(), &app, request.revision, &request.backend_digest).await?;
    let manifest = crate::manifest::load_manifest_json(rt.pool(), &app).await?
        .ok_or_else(|| ApiError::NotFound(app.clone()))?;
    let contract: rootcx_types::AppManifest = serde_json::from_value(manifest.clone())
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    super::validate(&contract)?;
    let authority = contract.actions.iter().find(|a| a.id == action).and_then(|a| a.authority.as_ref())
        .ok_or_else(|| ApiError::BadRequest("action authority missing".into()))?;
    let permissions: Vec<String> = authority.data.iter().flat_map(|(entity, ops)| {
        let app = &app;
        ops.iter().map(move |op| format!("app:{app}:{entity}.{}", op.as_str()))
    }).collect();
    let id = Uuid::new_v4();
    let artifact = super::artifact_dir(rt.data_dir(), request.revision);
    super::verify_artifact(&artifact, &request.backend_digest)
        .await.map_err(|e| ApiError::BadRequest(e.to_string()))?;
    crate::worker_manager::resolve_entry_point(&artifact)
        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
    if declaration["status"] == "approved" {
        return Ok(Json(declaration.clone()));
    }
    let mut tx = rt.pool().begin().await?;
    let old: Vec<Uuid> = sqlx::query_scalar(
        "UPDATE rootcx_system.action_approvals
         SET revoked_at=now(),revoked_by=$3,revocation_reason='approval replaced'
         WHERE app_id=$1 AND action_id=$2 AND revoked_at IS NULL RETURNING id",
    ).bind(&app).bind(&action).bind(identity.user_id).fetch_all(&mut *tx).await?;
    for old in old { super::sql::drop_role(&mut tx, old).await?; }
    super::sql::create_role(&mut tx, id, &contract, authority).await?;
    sqlx::query(
        "INSERT INTO rootcx_system.action_approvals
         (id,app_id,action_id,installation_id,revision,backend_digest,manifest,permissions,approved_by)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)",
    ).bind(id).bind(&app).bind(&action).bind(request.installation_id).bind(request.revision)
        .bind(request.backend_digest).bind(manifest).bind(permissions).bind(identity.user_id)
        .execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Json(json!({"id": action, "approvalId": id, "status":"approved"})))
}

async fn revoke(
    identity: Identity, State(rt): State<SharedRuntime>, Path((app, action)): Path<(String,String)>,
) -> Result<Json<Value>, ApiError> {
    crate::governance::authority::require_admin(rt.pool(), identity.user_id).await?;
    let _lifecycle = crate::governance::cross_app::lock_app_lifecycle(rt.pool(), &app).await?;
    // The update waits for every admitted action transaction's shared lock.
    let mut tx = rt.pool().begin().await?;
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "UPDATE rootcx_system.action_approvals
         SET revoked_at=now(),revoked_by=$3,revocation_reason='operator revoked'
         WHERE app_id=$1 AND action_id=$2 AND revoked_at IS NULL RETURNING id",
    ).bind(&app).bind(&action).bind(identity.user_id).fetch_all(&mut *tx).await?;
    for id in ids { super::sql::drop_role(&mut tx, id).await?; }
    tx.commit().await?;
    Ok(Json(json!({"id":action,"status":"pending"})))
}
