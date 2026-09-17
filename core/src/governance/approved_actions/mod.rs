//! Approval binds executable code to a data ceiling. Business decisions belong
//! to that reviewed code; neither callers nor ordinary workers inherit its grant.
use std::collections::HashSet;

use rootcx_types::AppManifest;
use serde_json::Value;
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use crate::RuntimeError;
use crate::governance::enforcement::{ContextState, InvocationContext};

mod artifacts;
mod routes;
mod sql;

pub(crate) use artifacts::{artifact_dir, begin_deployment, finish_deployment};
pub(crate) use artifacts::verify_artifact;
pub(crate) use routes::routes;

#[derive(Debug, Clone)]
pub struct ApprovedActionExecution {
    pub(crate) approval_id: Uuid,
    pub(crate) installation_id: Uuid,
    pub(crate) revision: Uuid,
    pub(crate) app_id: String,
    pub(crate) action_id: String,
    pub(crate) backend_digest: String,
    pub(crate) manifest: Value,
    pub(crate) execution_id: Uuid,
}

pub(crate) fn validate(manifest: &AppManifest) -> Result<(), RuntimeError> {
    let invalid = |message| RuntimeError::Invalid(format!("action authority: {message}"));
    let mut ids = HashSet::new();
    for action in &manifest.actions {
        if !ids.insert(&action.id) {
            return Err(invalid(format!("duplicate action '{}'", action.id)));
        }
        let Some(authority) = &action.authority else { continue };
        if authority.data.is_empty() {
            return Err(invalid(format!("'{}' must request at least one local entity", action.id)));
        }
        if manifest.public.as_ref().is_some_and(|p| p.rpcs.iter().any(|r| r.name == action.id)) {
            return Err(invalid(format!("'{}' cannot also be a public RPC", action.id)));
        }
        for (entity, operations) in &authority.data {
            if !manifest.data_contract.iter().any(|e| e.entity_name == *entity) {
                return Err(invalid(format!("'{}' references unknown local entity '{entity}'", action.id)));
            }
            if operations.is_empty() || operations.iter().enumerate().any(|(i, op)| operations[..i].contains(op)) {
                return Err(invalid(format!("'{}': '{entity}' needs nonempty, unique CRUD operations", action.id)));
            }
        }
    }
    Ok(())
}

pub(crate) async fn bootstrap(pool: &PgPool) -> Result<(), RuntimeError> {
    sqlx::raw_sql(include_str!("schema.sql")).execute(pool).await.map_err(RuntimeError::Schema)?;
    Ok(())
}

pub(crate) async fn install_gate(pool: &PgPool) -> Result<(), RuntimeError> {
    sqlx::raw_sql(include_str!("gate.sql")).execute(pool).await.map_err(RuntimeError::Schema)?;
    Ok(())
}

pub(crate) async fn revoke_app(
    tx: &mut Transaction<'_, Postgres>, app_id: &str, actor: Option<Uuid>, reason: &str,
) -> Result<(), sqlx::Error> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "UPDATE rootcx_system.action_approvals
            SET revoked_at = now(), revoked_by = $2, revocation_reason = $3
          WHERE app_id = $1 AND revoked_at IS NULL RETURNING id",
    ).bind(app_id).bind(actor).bind(reason).fetch_all(&mut **tx).await?;
    for id in ids { sql::drop_role(tx, id).await?; }
    Ok(())
}

pub(crate) fn execution_role(execution: &ApprovedActionExecution) -> String {
    sql::role_name(execution.approval_id)
}

// Reused at dispatch and process restart. No value is accepted from worker IPC.
const CURRENT_APPROVAL: &str =
    "SELECT g.* FROM rootcx_system.action_approvals g
     JOIN rootcx_system.backend_releases r
       ON r.app_id=g.app_id AND r.revision=g.revision AND r.digest=g.backend_digest
     JOIN rootcx_system.app_installations i
       ON i.id=g.installation_id AND i.app_id=g.app_id AND i.active
     JOIN rootcx_system.apps a
       ON a.id=g.app_id AND a.manifest=g.manifest AND a.status IN ('installed','system')
     WHERE g.revoked_at IS NULL";

pub(crate) async fn resolve(
    pool: &PgPool, app_id: &str, action_id: &str, caller: &ContextState,
) -> Result<Option<ApprovedActionExecution>, RuntimeError> {
    let manifest = crate::manifest::load_manifest_json(pool, app_id).await?
        .ok_or_else(|| RuntimeError::Invalid(format!("app '{app_id}' is not installed")))?;
    let requested = manifest["actions"].as_array().and_then(|actions| {
        actions.iter().find(|a| a["id"] == action_id)
    }).is_some_and(|action| !action["authority"].is_null());
    if !requested { return Ok(None); }
    let user = caller.user_id.ok_or_else(|| RuntimeError::PermissionDenied("approved actions require an authenticated caller".into()))?;
    let key = format!("app:{app_id}:action:{action_id}");
    let (_, permissions) = crate::governance::authority::resolve_permissions(pool, user)
        .await.map_err(|e| RuntimeError::Invalid(format!("{e:?}")))?;
    if !crate::governance::authority::has_permission(&permissions, &key)
        || (caller.is_delegated && !crate::governance::authority::has_permission(&caller.effective_perms, &key))
    {
        return Err(RuntimeError::PermissionDenied(format!("{key}; invoke alone does not authorize approved actions")));
    }
    let row = sqlx::query(&format!("{CURRENT_APPROVAL} AND g.app_id=$1 AND g.action_id=$2"))
        .bind(app_id).bind(action_id).fetch_optional(pool).await.map_err(RuntimeError::Schema)?
        .ok_or_else(|| RuntimeError::PermissionDenied(format!(
            "action '{action_id}' requires approval of the current backend; inspect /api/v1/apps/{app_id}/action-approvals"
        )))?;
    Ok(Some(ApprovedActionExecution {
        approval_id: row.get("id"), installation_id: row.get("installation_id"),
        revision: row.get("revision"), app_id: app_id.into(), action_id: action_id.into(),
        backend_digest: row.get("backend_digest"), manifest: row.get("manifest"),
        execution_id: Uuid::new_v4(),
    }))
}

pub(crate) async fn verify_execution(
    pool: &PgPool, execution: &ApprovedActionExecution,
) -> Result<(), RuntimeError> {
    let exists = sqlx::query(&format!("{CURRENT_APPROVAL} AND g.id=$1"))
        .bind(execution.approval_id).fetch_optional(pool).await.map_err(RuntimeError::Schema)?.is_some();
    if !exists {
        return Err(RuntimeError::Invalid("approved action was revoked or its release changed".into()));
    }
    Ok(())
}

pub(crate) async fn bind_context(
    tx: &mut Transaction<'_, Postgres>, app: &str, state: &ContextState, invocation: &InvocationContext,
) -> Result<(), sqlx::Error> {
    let Some(execution) = &state.approved_action else { return Ok(()) };
    let fail = || sqlx::Error::Protocol("approved action authority is no longer valid".into());
    if app != execution.app_id || invocation != &InvocationContext::action(&execution.action_id) {
        return Err(fail());
    }
    // Same lock order as lifecycle transitions. Revocation waits for admitted
    // transactions; a completed revocation cannot be followed by a stale commit.
    sqlx::query("SELECT id FROM rootcx_system.app_installations WHERE id=$1 FOR SHARE")
        .bind(execution.installation_id).fetch_optional(&mut **tx).await?.ok_or_else(fail)?;
    sqlx::query("SELECT revision FROM rootcx_system.backend_releases WHERE app_id=$1 FOR SHARE")
        .bind(app).fetch_optional(&mut **tx).await?.ok_or_else(fail)?;
    let row = sqlx::query(
        "SELECT permissions FROM rootcx_system.action_approvals WHERE id=$1 FOR SHARE",
    ).bind(execution.approval_id).fetch_optional(&mut **tx).await?.ok_or_else(fail)?;
    sqlx::query("SELECT set_config('rootcx.approved_action_id', $1, true)")
        .bind(execution.approval_id.to_string()).execute(&mut **tx).await?;
    let permissions: Vec<String> = row.get("permissions");
    let valid: bool = sqlx::query_scalar("SELECT rootcx_system.check_approved_action_access($1)")
        .bind(permissions.first().ok_or_else(fail)?).fetch_one(&mut **tx).await?;
    if !valid { return Err(fail()); }
    Ok(())
}

pub(crate) async fn start_execution(
    pool: &PgPool, execution: &ApprovedActionExecution, state: &ContextState,
) -> Result<Uuid, RuntimeError> {
    sqlx::query(
        "INSERT INTO rootcx_system.action_executions (id,approval_id,user_id,actor_id,delegator_id)
         VALUES ($1,$2,$3,$4,$5)",
    ).bind(execution.execution_id).bind(execution.approval_id).bind(state.user_id)
        .bind(state.audit_actor_id.or(state.user_id)).bind(state.audit_delegator_id).execute(pool).await.map_err(RuntimeError::Schema)?;
    Ok(execution.execution_id)
}

pub(crate) async fn finish_execution(
    pool: &PgPool, id: Uuid, success: bool, error: Option<&str>,
) -> Result<(), RuntimeError> {
    sqlx::query(
        "UPDATE rootcx_system.action_executions SET finished_at=now(),success=$2,error=$3 WHERE id=$1",
    ).bind(id).bind(success).bind(error.map(|s| s.chars().take(2048).collect::<String>())).execute(pool).await.map_err(RuntimeError::Schema)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn authority_rejects_ambiguous_or_nonlocal_requests() {
        let base = json!({
            "appId":"support", "name":"Support",
            "actions":[{"id":"record","name":"Record","authority":{"data":{"case_file":["read"]}}}],
            "dataContract":[{"entityName":"case_file","fields":[]}]
        });
        for (name, change) in [
            ("empty", json!({"data":{}})),
            ("empty verbs", json!({"data":{"case_file":[]}})),
            ("duplicate verbs", json!({"data":{"case_file":["read","read"]}})),
            ("unknown entity", json!({"data":{"missing":["read"]}})),
            ("foreign entity", json!({"data":{"other:case_file":["read"]}})),
        ] {
            let mut value = base.clone();
            value["actions"][0]["authority"] = change;
            let manifest: AppManifest = serde_json::from_value(value).unwrap();
            assert!(validate(&manifest).is_err(), "{name}");
        }
        let manifest: AppManifest = serde_json::from_value(base.clone()).unwrap();
        assert!(validate(&manifest).is_ok());
        for duplicate in [false, true] {
            let mut value = base.clone();
            if duplicate {
                let action = value["actions"][0].clone();
                value["actions"].as_array_mut().unwrap().push(action);
            } else {
                value["public"] = json!({"rpcs":[{"name":"record"}]});
            }
            let manifest: AppManifest = serde_json::from_value(value).unwrap();
            assert!(validate(&manifest).is_err(), "duplicate={duplicate}");
        }
    }
}
