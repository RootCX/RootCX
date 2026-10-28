//! One installation-bound, confirmed lifecycle operation for humans and agents.
mod routes;
mod runner;
use crate::{RuntimeError, api_error::ApiError, auth::identity::principal_enabled};
pub(crate) use routes::availability;
pub use routes::routes;
pub use runner::spawn;
use serde::Serialize;
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

pub const PERMISSION: &str = "admin:apps.delete";

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Deletion {
    pub id: Uuid,
    pub app_id: String,
    pub installation_id: Uuid,
    pub application_name: String,
    pub requested_by: Uuid,
    pub agent_app: Option<String>,
    pub state: String,
    pub phase: String,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub confirmed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub attempts: i32,
    pub error: Option<String>,
}

pub async fn authorize(pool: &PgPool, human: Uuid) -> Result<(), ApiError> {
    if !principal_enabled(pool, human).await {
        return Err(ApiError::Unauthorized(
            "responsible human disabled or missing".into(),
        ));
    }
    super::authority::require_perm(pool, human, PERMISSION).await
}

pub async fn ensure_available(pool: &PgPool, app: &str) -> Result<(), RuntimeError> {
    let pending: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM rootcx_system.app_deletions WHERE app_id=$1 AND state IN ('deleting','retry'))")
        .bind(app).fetch_one(pool).await.map_err(RuntimeError::Schema)?;
    if pending {
        return Err(RuntimeError::Conflict(
            "application deletion is in progress".into(),
        ));
    }
    Ok(())
}

pub async fn get(pool: &PgPool, id: Uuid) -> Result<Deletion, ApiError> {
    sqlx::query_as("SELECT * FROM rootcx_system.app_deletions WHERE id=$1")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| ApiError::NotFound("deletion operation not found".into()))
}

pub async fn prepare(
    pool: &PgPool,
    app: &str,
    human: Uuid,
    agent: Option<&str>,
) -> Result<Deletion, ApiError> {
    authorize(pool, human).await?;
    crate::manifest::validate_new_ident(app, "app_id")?;
    if crate::manifest::RESERVED_SCHEMAS.contains(&app) || app.starts_with("pg_") {
        return Err(ApiError::BadRequest("reserved application schema".into()));
    }
    let mut lifecycle = super::cross_app::lock_app_lifecycle(pool, app).await?;
    ensure_available(pool, app).await?;
    let mut tx = lifecycle.begin().await?;
    let target: Option<(Uuid, String)> = sqlx::query_as(
        "SELECT i.id,a.name FROM rootcx_system.apps a JOIN rootcx_system.app_installations i ON i.app_id=a.id AND i.active
         WHERE a.id=$1 AND a.status='installed' AND COALESCE(a.manifest->>'type','app') = 'app'
           AND NOT EXISTS(SELECT 1 FROM rootcx_system.agents WHERE app_id=a.id)")
        .bind(app).fetch_optional(&mut *tx).await?;
    let (installation, name) =
        target.ok_or_else(|| ApiError::NotFound("installed application not found".into()))?;
    let operation: Deletion = sqlx::query_as("INSERT INTO rootcx_system.app_deletions(id,app_id,installation_id,application_name,requested_by,agent_app) VALUES($1,$2,$3,$4,$5,$6) RETURNING *")
        .bind(Uuid::new_v4()).bind(app).bind(installation).bind(name).bind(human).bind(agent).fetch_one(&mut *tx).await?;
    event(&mut tx, &operation, "requested", None).await?;
    tx.commit().await?;
    Ok(operation)
}

/// Once admitted, cleanup is a Core obligation; later permission changes do not
/// leave an already revoked installation half erased. Admission is revalidated.
pub async fn confirm(
    pool: &PgPool,
    id: Uuid,
    human: Uuid,
    authority: Option<&super::execution::AgentAuthority>,
) -> Result<Deletion, ApiError> {
    let result = confirm_inner(pool, id, human, authority).await;
    if result.is_err() {
        if let Ok(mut operation) = get(pool, id).await {
            operation.requested_by = human;
            operation.agent_app = authority.map(|authority| authority.app().into());
            let mut tx = pool.begin().await?;
            event(
                &mut tx,
                &operation,
                "confirmation_refused",
                Some("Authority, confirmation or application state rejected"),
            )
            .await?;
            tx.commit().await?;
        }
    }
    result
}

async fn confirm_inner(
    pool: &PgPool,
    id: Uuid,
    human: Uuid,
    authority: Option<&super::execution::AgentAuthority>,
) -> Result<Deletion, ApiError> {
    authorize(pool, human).await?;
    let agent = authority.map(|authority| authority.app());
    let initial = get(pool, id).await?;
    if let Some(authority) = authority {
        let args = serde_json::json!({"appId": initial.app_id, "applicationName": initial.application_name, "operationId": initial.id});
        if !authority.confirmed_call("delete_app", &args) {
            return Err(ApiError::Forbidden(
                "deletion requires a bound human confirmation".into(),
            ));
        }
    }
    if initial.requested_by != human || initial.agent_app.as_deref() != agent {
        return Err(ApiError::Forbidden(
            "confirmation belongs to another caller".into(),
        ));
    }
    let mut lifecycle = super::cross_app::lock_app_lifecycle(pool, &initial.app_id).await?;
    authorize(pool, human).await?;
    if let Some(authority) = authority {
        let permissions = authority
            .revalidate(pool)
            .await
            .map_err(ApiError::Forbidden)?;
        crate::tools::check_permission(&permissions, PERMISSION).map_err(ApiError::Forbidden)?;
        crate::tools::check_permission(&permissions, "tool:delete_app")
            .map_err(ApiError::Forbidden)?;
    }
    let mut tx = lifecycle.begin().await?;
    let mut operation: Deletion =
        sqlx::query_as("SELECT * FROM rootcx_system.app_deletions WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
    if operation.state != "awaiting_confirmation" {
        return Ok(operation);
    }
    let valid: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM rootcx_system.app_installations WHERE id=$1 AND app_id=$2 AND active) AND EXISTS(SELECT 1 FROM rootcx_system.app_deletions WHERE id=$3 AND expires_at > now())")
        .bind(operation.installation_id).bind(&operation.app_id).bind(id).fetch_one(&mut *tx).await?;
    if !valid {
        return Err(ApiError::Conflict(
            "confirmation expired or installation changed".into(),
        ));
    }
    // Source admission also locks this row. A queued or running change must
    // finish before deletion; no new change can enter during the transition.
    sqlx::query("SELECT app_id FROM rootcx_system.source_projects WHERE app_id=$1 FOR UPDATE")
        .bind(&operation.app_id)
        .execute(&mut *tx)
        .await?;
    let busy: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM rootcx_system.source_runs WHERE app_id=$1 AND status IN ('queued','coding','publishing','needs_recovery'))")
        .bind(&operation.app_id).fetch_one(&mut *tx).await?;
    if busy {
        return Err(ApiError::Conflict(
            "application has pending changes or needs recovery".into(),
        ));
    }
    super::cross_app::deactivate_installation_tx(
        &mut tx,
        &operation.app_id,
        Some(human),
        "application deletion confirmed",
    )
    .await?;
    sqlx::query("UPDATE rootcx_system.apps SET status='deleting' WHERE id=$1")
        .bind(&operation.app_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE rootcx_system.app_deletions SET state='deleting',confirmed_at=now(),updated_at=now() WHERE id=$1")
        .bind(id).execute(&mut *tx).await?;
    operation.state = "deleting".into();
    event(&mut tx, &operation, "confirmed", None).await?;
    tx.commit().await?;
    get(pool, id).await
}

pub(super) async fn event(
    tx: &mut Transaction<'_, Postgres>,
    operation: &Deletion,
    name: &str,
    detail: Option<&str>,
) -> Result<(), sqlx::Error> {
    let actor = operation
        .agent_app
        .as_deref()
        .map(crate::extensions::agents::agent_user_id)
        .unwrap_or(operation.requested_by);
    let delegator = operation.agent_app.as_ref().map(|_| operation.requested_by);
    super::audit::set_context(tx, Some(actor), delegator, &operation.id.to_string()).await?;
    sqlx::query("INSERT INTO rootcx_system.app_deletion_events(operation_id,event,phase,actor_uid,agent_app,detail,delegator_uid) VALUES($1,$2,$3,$4,$5,$6,$7)")
        .bind(operation.id).bind(name).bind(&operation.phase).bind(actor).bind(&operation.agent_app).bind(detail).bind(delegator)
        .execute(&mut **tx).await?;
    Ok(())
}
