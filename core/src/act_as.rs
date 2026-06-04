//! The single act-as gate. Authorizes a human to run a unit of work AS another
//! principal (a service account), used by every `run_as` site. There is exactly
//! one way to run as another principal: a standing act-as delegation bounded by
//! an anti-escalation subset check. See docs/service-accounts.md.

use sqlx::PgPool;
use uuid::Uuid;

use crate::RuntimeError;
use crate::api_error::ApiError;
use crate::extensions::rbac::policy::{has_permission, resolve_permissions};

/// Authorize `human` to act as `target`. Deny-by-default; both required:
///   1. a standing act-as delegation `human -> target` exists, AND
///   2. anti-escalation: every permission of `target` is held by `human`.
///      No bypass exists. The subset check always runs.
pub async fn assert_can_act_as(pool: &PgPool, human: Uuid, target: Uuid) -> Result<(), ApiError> {
    if human == target {
        return Ok(());
    }

    let delegated = crate::delegations::is_valid(pool, human, target)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    if !delegated {
        return Err(ApiError::Forbidden(format!("no act-as delegation for {target}")));
    }

    let (_, human_perms) = resolve_permissions(pool, human).await?;
    let (_, target_perms) = resolve_permissions(pool, target).await?;
    if target_perms.iter().all(|p| has_permission(&human_perms, p)) {
        Ok(())
    } else {
        Err(ApiError::Forbidden(
            "act-as denied: target permissions exceed yours (anti-escalation)".into(),
        ))
    }
}

/// Resolve the owner (`created_by`) of a unit of work. `run_as` is the optional
/// principal id the caller wants to own it; absent -> the caller. When present,
/// it passes the act-as gate. The single entry point for every `run_as` site.
pub async fn resolve_owner(pool: &PgPool, caller: Uuid, run_as: Option<&str>) -> Result<Uuid, ApiError> {
    match run_as {
        Some(s) => {
            let target: Uuid = s.parse().map_err(|_| ApiError::BadRequest("invalid run_as".into()))?;
            assert_can_act_as(pool, caller, target).await?;
            Ok(target)
        }
        None => Ok(caller),
    }
}

/// Grant a standing act-as delegation `human -> sa`. Idempotent.
pub async fn grant(pool: &PgPool, human: Uuid, sa: Uuid) -> Result<(), RuntimeError> {
    if crate::delegations::is_valid(pool, human, sa).await? {
        return Ok(());
    }
    crate::delegations::create(pool, human, sa, "act_as", None).await.map(|_| ())
}

/// Revoke every standing act-as delegation `human -> sa`.
pub async fn revoke(pool: &PgPool, human: Uuid, sa: Uuid) -> Result<(), RuntimeError> {
    sqlx::query(
        "UPDATE rootcx_system.delegations SET revoked_at = now() \
         WHERE delegator_uid = $1 AND delegatee_uid = $2 \
         AND trigger_type = 'act_as' AND revoked_at IS NULL",
    )
    .bind(human)
    .bind(sa)
    .execute(pool)
    .await
    .map_err(RuntimeError::Schema)?;
    Ok(())
}
