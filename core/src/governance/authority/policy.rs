use std::collections::HashMap;

use sqlx::PgPool;
use uuid::Uuid;

use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use super::perms::{expand_roles, has_permission, intersect_permissions};

pub async fn resolve_permissions(pool: &PgPool, user_id: Uuid) -> Result<(Vec<String>, Vec<String>), ApiError> {
    let assigned: Vec<(String,)> =
        sqlx::query_as("SELECT role FROM rootcx_system.rbac_assignments WHERE user_id = $1")
            .bind(user_id).fetch_all(pool).await?;

    let assigned_names: Vec<String> = assigned.into_iter().map(|(r,)| r).collect();
    if assigned_names.is_empty() {
        return Ok((vec![], vec![]));
    }

    let rows: Vec<(String, Vec<String>, Vec<String>)> = sqlx::query_as(
        "SELECT name, inherits, permissions FROM rootcx_system.rbac_roles",
    ).fetch_all(pool).await?;

    let mut hierarchy = HashMap::with_capacity(rows.len());
    let mut role_perms = HashMap::with_capacity(rows.len());
    for (name, inherits, perms) in rows {
        hierarchy.insert(name.clone(), inherits);
        role_perms.insert(name, perms);
    }

    let expanded = expand_roles(&assigned_names, &hierarchy);

    let mut perm_set = std::collections::HashSet::new();
    for role_name in &expanded {
        if let Some(perms) = role_perms.get(role_name) {
            perm_set.extend(perms.iter().cloned());
        }
    }

    let mut roles: Vec<String> = expanded.into_iter().collect();
    roles.sort_unstable();
    let mut permissions: Vec<String> = perm_set.into_iter().collect();
    permissions.sort_unstable();
    Ok((roles, permissions))
}

pub async fn has_permission_db(pool: &PgPool, user_id: Uuid, required: &str) -> Result<bool, ApiError> {
    Ok(sqlx::query_scalar::<_, bool>("SELECT rootcx_system.has_permission($1, $2)")
        .bind(user_id).bind(required).fetch_one(pool).await?)
}

pub async fn require_perm(pool: &PgPool, user_id: Uuid, perm: &str) -> Result<(), ApiError> {
    if has_permission_db(pool, user_id, perm).await? { Ok(()) }
    else { Err(ApiError::Forbidden(format!("permission denied: {perm}"))) }
}

pub async fn require_admin(pool: &PgPool, user_id: Uuid) -> Result<(), ApiError> {
    let (_, perms) = resolve_permissions(pool, user_id).await?;
    if perms.iter().any(|p| p == "*") { Ok(()) }
    else { Err(ApiError::Forbidden("admin access required".into())) }
}

pub async fn resolve_effective_permissions(pool: &PgPool, _identity: &Identity) -> Result<Vec<String>, ApiError> {
    let (_, perms) = resolve_permissions(pool, _identity.user_id).await?;
    Ok(perms)
}

pub async fn effective_for_pair(pool: &PgPool, agent_uid: Uuid, delegator_uid: Uuid) -> Vec<String> {
    let (agent_res, deleg_res) = tokio::join!(
        resolve_permissions(pool, agent_uid),
        resolve_permissions(pool, delegator_uid),
    );
    let agent_perms = agent_res.map(|(_, p)| p).unwrap_or_default();
    let deleg_perms = deleg_res.map(|(_, p)| p).unwrap_or_default();
    intersect_permissions(&agent_perms, &deleg_perms)
}

pub async fn effective_under_parent(pool: &PgPool, child_agent_uid: Uuid, parent_perms: &[String]) -> Vec<String> {
    let child = resolve_permissions(pool, child_agent_uid).await.map(|(_, p)| p).unwrap_or_default();
    intersect_permissions(&child, parent_perms)
}

pub async fn delegated_effective(
    pool: &PgPool, agent_uid: Uuid,
    invoker_user_id: Option<Uuid>, parent_perms: Option<&[String]>,
) -> Vec<String> {
    match parent_perms {
        Some(pp) => effective_under_parent(pool, agent_uid, pp).await,
        None => match invoker_user_id {
            Some(uid) => effective_for_pair(pool, agent_uid, uid).await,
            None => vec![],
        },
    }
}
