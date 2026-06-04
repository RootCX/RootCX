use sqlx::PgPool;
use uuid::Uuid;

pub enum FireDenied {
    NoOwner,
    Disabled,
    NoDelegation,
    NoInvoke(String),
    CheckFailed(String),
}

/// Unified fire-time gate (governance-philosophy.md, B1).
/// Every owned-automation trigger must pass ALL checks:
/// 1. Owner exists
/// 2. Owner is enabled
/// 3. Valid delegation grant exists
/// 4. Owner still holds app:{id}:invoke
pub async fn assert_can_fire(pool: &PgPool, owner: Option<Uuid>, app_id: &str) -> Result<Uuid, FireDenied> {
    let delegator = owner.ok_or(FireDenied::NoOwner)?;

    let enabled: bool = sqlx::query_scalar(
        "SELECT disabled_at IS NULL FROM rootcx_system.users WHERE id = $1"
    ).bind(delegator).fetch_optional(pool).await
        .ok().flatten().unwrap_or(false);
    if !enabled {
        return Err(FireDenied::Disabled);
    }

    let agent_uid = crate::extensions::agents::agent_user_id(app_id);
    match crate::delegations::is_valid(pool, delegator, agent_uid).await {
        Ok(true) => {}
        Ok(false) => return Err(FireDenied::NoDelegation),
        Err(e) => return Err(FireDenied::CheckFailed(e.to_string())),
    }

    let required_perm = format!("app:{app_id}:invoke");
    let (_, perms) = crate::extensions::rbac::policy::resolve_permissions(pool, delegator)
        .await
        .map_err(|e| FireDenied::CheckFailed(format!("{e:?}")))?;
    if !crate::extensions::rbac::policy::has_permission(&perms, &required_perm) {
        return Err(FireDenied::NoInvoke(required_perm));
    }

    Ok(delegator)
}
