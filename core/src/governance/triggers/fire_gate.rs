//! The single delegation-gated fire check (governance-philosophy.md, B1 + B3 / invariant #5).
//!
//! Cron, hook and webhook are *owned automations*: a non-human run firing under a
//! stored human's authority. A channel message is *attended* (the linked user is
//! the responsible principal directly, like a UI click), but is still gated by the
//! channel-link delegation grant. All four therefore share ONE check: the
//! responsible principal is present AND enabled AND has a valid delegation AND
//! still holds `app:{id}:invoke` — else refused, fail-closed. (Task scope, which
//! DOES differ — `app:{id}:*` for owned automations, `None` for attended channel
//! runs — is set by each caller, never here.)
//!
//! This is the only place those four checks live. Every fire site calls
//! `assert_can_fire`; none re-derives the rule. That is what keeps invariant #2
//! ("disabled = denied instantly, no path exempt") true on every trigger path.

use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::identity::principal_enabled;
use crate::governance::authority::policy::has_permission_db;
use crate::governance::delegation::grants::is_valid;

/// Why a trigger was refused. Each fire site maps this to its own surface
/// (job failure + log, HTTP error, channel mute) — the gate decides, the caller
/// translates.
pub enum FireDenied {
    /// No responsible principal (e.g. `created_by` is NULL / no linked user).
    NoOwner,
    /// The owner is disabled or no longer exists.
    Disabled,
    /// No valid delegation grant from the owner to this app's agent.
    NoDelegation,
    /// The owner no longer holds `app:{id}:invoke`.
    NoInvoke(String),
    /// A dependency (DB) failed while checking; fail-closed.
    CheckFailed(String),
}

/// A short, log-ready reason. Callers append their own context (msg id, app).
impl std::fmt::Display for FireDenied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FireDenied::NoOwner => write!(f, "no responsible owner"),
            FireDenied::Disabled => write!(f, "owner disabled or missing"),
            FireDenied::NoDelegation => write!(f, "no valid delegation"),
            FireDenied::NoInvoke(perm) => write!(f, "owner lacks {perm}"),
            FireDenied::CheckFailed(e) => write!(f, "gate check failed: {e}"),
        }
    }
}

/// Authorize an owned automation to fire. Deny-by-default; ALL checks run, in
/// this order:
///   1. owner exists,
///   2. owner enabled,
///   3. a valid delegation grant `owner -> app's agent` exists,
///   4. owner still holds `app:{id}:invoke`.
///
/// On success returns the responsible principal (the owner / delegator) so the
/// caller can attribute the run without re-resolving it.
pub async fn assert_can_fire(pool: &PgPool, owner: Option<Uuid>, app_id: &str) -> Result<Uuid, FireDenied> {
    let delegator = owner.ok_or(FireDenied::NoOwner)?;

    if !principal_enabled(pool, delegator).await {
        return Err(FireDenied::Disabled);
    }

    let agent_uid = crate::extensions::agents::agent_user_id(app_id);
    match is_valid(pool, delegator, agent_uid).await {
        Ok(true) => {}
        Ok(false) => return Err(FireDenied::NoDelegation),
        Err(e) => return Err(FireDenied::CheckFailed(e.to_string())),
    }

    // Use the same SQL authority oracle (`rootcx_system.has_permission`) the RLS
    // data layer checks against — one round-trip, and the gate can never drift
    // from what the data layer enforces.
    let required_perm = format!("app:{app_id}:invoke");
    let holds = has_permission_db(pool, delegator, &required_perm)
        .await
        .map_err(|e| FireDenied::CheckFailed(format!("{e:?}")))?;
    if !holds {
        return Err(FireDenied::NoInvoke(required_perm));
    }

    Ok(delegator)
}

/// Lighter gate for workflows: owner present + enabled + holds invoke.
/// Skips the delegation check because workflows have no agent principal
/// to delegate to (the automation runs directly under the owner's identity).
pub async fn assert_can_fire_workflow(pool: &PgPool, owner: Option<Uuid>, app_id: &str) -> Result<Uuid, FireDenied> {
    let uid = owner.ok_or(FireDenied::NoOwner)?;
    if !principal_enabled(pool, uid).await {
        return Err(FireDenied::Disabled);
    }
    let required_perm = format!("app:{app_id}:invoke");
    let holds = has_permission_db(pool, uid, &required_perm)
        .await
        .map_err(|e| FireDenied::CheckFailed(format!("{e:?}")))?;
    if !holds {
        return Err(FireDenied::NoInvoke(required_perm));
    }
    Ok(uid)
}
