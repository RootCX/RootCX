use std::collections::{HashMap, HashSet};

use sqlx::PgPool;
use uuid::Uuid;

use crate::api_error::ApiError;
use crate::auth::identity::Identity;

const MAX_ROLE_DEPTH: usize = 64;

pub fn expand_roles(assigned: &[String], role_map: &HashMap<String, Vec<String>>) -> HashSet<String> {
    let mut expanded = HashSet::new();
    let mut stack: Vec<&str> = assigned.iter().map(|s| s.as_str()).collect();
    let mut depth = 0usize;
    while let Some(role) = stack.pop() {
        depth += 1;
        if depth > MAX_ROLE_DEPTH { break; }
        if expanded.insert(role.to_string())
            && let Some(parents) = role_map.get(role) {
                for parent in parents {
                    if !expanded.contains(parent.as_str()) {
                        stack.push(parent);
                    }
                }
            }
    }
    expanded
}

pub fn detect_cycle(roles: &HashMap<String, Vec<String>>) -> Option<String> {
    let mut visited = HashSet::new();
    let mut in_stack = HashSet::new();
    for role in roles.keys() {
        if !visited.contains(role.as_str())
            && let Some(cycle) = dfs_cycle(role, roles, &mut visited, &mut in_stack) {
                return Some(cycle);
            }
    }
    None
}

fn dfs_cycle<'a>(
    node: &'a str,
    roles: &'a HashMap<String, Vec<String>>,
    visited: &mut HashSet<&'a str>,
    in_stack: &mut HashSet<&'a str>,
) -> Option<String> {
    visited.insert(node);
    in_stack.insert(node);
    if let Some(parents) = roles.get(node) {
        for parent in parents {
            if in_stack.contains(parent.as_str()) {
                return Some(parent.clone());
            }
            if !visited.contains(parent.as_str())
                && let Some(cycle) = dfs_cycle(parent, roles, visited, in_stack) {
                    return Some(cycle);
                }
        }
    }
    in_stack.remove(node);
    None
}

/// Resolve all permissions for a user (global — no app scoping).
pub async fn resolve_permissions(
    pool: &PgPool,
    user_id: Uuid,
) -> Result<(Vec<String>, Vec<String>), ApiError> {
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

    let mut perm_set = HashSet::new();
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

pub async fn require_admin(pool: &PgPool, user_id: Uuid) -> Result<(), ApiError> {
    let (_, perms) = resolve_permissions(pool, user_id).await?;
    if perms.iter().any(|p| p == "*") { Ok(()) }
    else { Err(ApiError::Forbidden("admin access required".into())) }
}

/// Effective permissions for an authenticated request.
/// Delegated token (act present): grant(agent) ∩ perms(delegator).
/// Direct request: the user's own permissions.
pub async fn resolve_effective_permissions(pool: &PgPool, identity: &Identity) -> Result<Vec<String>, ApiError> {
    let Some(actor) = &identity.actor else {
        let (_, perms) = resolve_permissions(pool, identity.user_id).await?;
        return Ok(perms);
    };
    let agent_uid: Uuid = actor.sub.parse()
        .map_err(|_| ApiError::Unauthorized("invalid actor subject".into()))?;
    let (agent_res, delegator_res) = tokio::join!(
        resolve_permissions(pool, agent_uid),
        resolve_permissions(pool, identity.user_id),
    );
    let (_, agent_perms) = agent_res?;
    let (_, delegator_perms) = delegator_res?;
    Ok(intersect_permissions(&agent_perms, &delegator_perms))
}

/// Effective permissions for an agent invocation with no Identity in scope
/// (scheduler/worker path). Deny-on-error: a failed RBAC lookup yields no authority.
pub async fn effective_for_pair(pool: &PgPool, agent_uid: Uuid, delegator_uid: Uuid) -> Vec<String> {
    let (agent_res, deleg_res) = tokio::join!(
        resolve_permissions(pool, agent_uid),
        resolve_permissions(pool, delegator_uid),
    );
    let agent_perms = agent_res.map(|(_, p)| p).unwrap_or_default();
    let deleg_perms = deleg_res.map(|(_, p)| p).unwrap_or_default();
    intersect_permissions(&agent_perms, &deleg_perms)
}

/// Compute the intersection of two permission sets.
/// A permission is in the result IFF both sides grant it.
/// Handles wildcards: `*` grants everything, `ns:scope:*` grants the subtree.
pub fn intersect_permissions(a: &[String], b: &[String]) -> Vec<String> {
    if a.iter().any(|p| p == "*") { return b.to_vec(); }
    if b.iter().any(|p| p == "*") { return a.to_vec(); }
    let mut result: Vec<String> = a.iter()
        .filter(|p| has_permission(b, p))
        .cloned()
        .collect();
    for p in b {
        if has_permission(a, p) && !result.contains(p) {
            result.push(p.clone());
        }
    }
    result.sort_unstable();
    result.dedup();
    result
}

/// Check if a user has a specific permission. Supports:
/// - `*` global admin
/// - `{namespace}:{scope}:*` scoped wildcard (e.g. `app:crm:*`, `tool:*`, `integration:gmail:*`)
pub fn has_permission(permissions: &[String], required: &str) -> bool {
    permissions.iter().any(|p| {
        p == "*" || p == required || {
            if let Some(prefix) = p.strip_suffix(":*") {
                required.starts_with(prefix) && required.as_bytes().get(prefix.len()) == Some(&b':')
            } else {
                false
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roles(entries: &[(&str, &[&str])]) -> HashMap<String, Vec<String>> {
        entries.iter().map(|(k, v)| (k.to_string(), v.iter().map(|s| s.to_string()).collect())).collect()
    }

    #[test]
    fn expand_roles_simple() {
        let r = roles(&[("admin", &[]), ("editor", &["viewer"]), ("viewer", &[])]);
        let expanded = expand_roles(&["editor".into()], &r);
        assert!(expanded.contains("editor") && expanded.contains("viewer") && !expanded.contains("admin"));
    }

    #[test]
    fn expand_roles_transitive() {
        let r = roles(&[("admin", &["editor"]), ("editor", &["viewer"]), ("viewer", &[])]);
        assert_eq!(expand_roles(&["admin".into()], &r).len(), 3);
    }

    #[test]
    fn detect_cycle_none() { assert!(detect_cycle(&roles(&[("a", &["b"]), ("b", &["c"]), ("c", &[])])).is_none()); }

    #[test]
    fn detect_cycle_direct() { assert!(detect_cycle(&roles(&[("a", &["b"]), ("b", &["a"])])).is_some()); }

    #[test]
    fn detect_cycle_indirect() { assert!(detect_cycle(&roles(&[("a", &["b"]), ("b", &["c"]), ("c", &["a"])])).is_some()); }

    #[test]
    fn expand_roles_empty() { assert!(expand_roles(&[], &roles(&[("admin", &[])])).is_empty()); }

    #[test]
    fn expand_roles_unknown() {
        let expanded = expand_roles(&["ghost".into()], &roles(&[("admin", &[])]));
        assert_eq!(expanded.len(), 1);
        assert!(expanded.contains("ghost"));
    }

    #[test]
    fn detect_cycle_self() { assert!(detect_cycle(&roles(&[("a", &["a"])])).is_some()); }

    #[test]
    fn expand_roles_diamond() {
        let r = roles(&[("admin", &["editor", "reviewer"]), ("editor", &["viewer"]), ("reviewer", &["viewer"]), ("viewer", &[])]);
        assert_eq!(expand_roles(&["admin".into()], &r).len(), 4);
    }

    #[test]
    fn detect_cycle_disconnected() {
        assert!(detect_cycle(&roles(&[("a", &["b"]), ("b", &[]), ("x", &["y"]), ("y", &[])])).is_none());
    }

    #[test]
    fn intersect_both_concrete_overlap() {
        let a = vec!["app:crm:customer.read".into(), "app:crm:customer.write".into(), "tool:query_data".into()];
        let b = vec!["app:crm:customer.read".into(), "tool:query_data".into(), "tool:mutate_data".into()];
        let result = intersect_permissions(&a, &b);
        assert_eq!(result, vec!["app:crm:customer.read", "tool:query_data"]);
    }

    #[test]
    fn intersect_no_overlap_is_empty() {
        let a = vec!["app:crm:customer.read".into()];
        let b = vec!["app:support:ticket.read".into()];
        assert!(intersect_permissions(&a, &b).is_empty());
    }

    #[test]
    fn intersect_one_side_global_wildcard() {
        let a = vec!["*".into()];
        let b = vec!["app:crm:customer.read".into(), "tool:query_data".into()];
        assert_eq!(intersect_permissions(&a, &b), b);
        assert_eq!(intersect_permissions(&b, &a), vec!["app:crm:customer.read", "tool:query_data"]);
    }

    #[test]
    fn intersect_both_global_wildcard() {
        assert_eq!(intersect_permissions(&["*".into()], &["*".into()]), vec!["*"]);
    }

    #[test]
    fn intersect_scoped_wildcard_vs_concrete() {
        let a = vec!["app:crm:*".into()];
        let b = vec!["app:crm:customer.read".into(), "app:support:ticket.read".into()];
        let result = intersect_permissions(&a, &b);
        assert_eq!(result, vec!["app:crm:customer.read"]);
    }

    #[test]
    fn intersect_scoped_wildcard_different_ns() {
        let a = vec!["app:crm:*".into()];
        let b = vec!["app:support:ticket.read".into()];
        assert!(intersect_permissions(&a, &b).is_empty());
    }

    #[test]
    fn intersect_both_empty() {
        let empty: Vec<String> = vec![];
        assert!(intersect_permissions(&empty, &empty).is_empty());
    }

    #[test]
    fn intersect_one_empty() {
        let a = vec!["app:crm:customer.read".into()];
        let empty: Vec<String> = vec![];
        assert!(intersect_permissions(&a, &empty).is_empty());
        assert!(intersect_permissions(&empty, &a).is_empty());
    }

    #[test]
    fn has_permission_exact() {
        assert!(has_permission(&["app:crm:customer.read".into()], "app:crm:customer.read"));
        assert!(!has_permission(&["app:crm:customer.read".into()], "app:crm:customer.write"));
    }

    #[test]
    fn has_permission_global_wildcard() {
        assert!(has_permission(&["*".into()], "app:crm:customer.read"));
        assert!(has_permission(&["*".into()], "tool:query_data"));
    }

    #[test]
    fn has_permission_app_wildcard() {
        assert!(has_permission(&["app:crm:*".into()], "app:crm:customer.read"));
        assert!(!has_permission(&["app:crm:*".into()], "app:support:ticket.read"));
        assert!(!has_permission(&["app:crm:*".into()], "tool:query_data"));
    }

    #[test]
    fn has_permission_tool_wildcard() {
        assert!(has_permission(&["tool:*".into()], "tool:query_data"));
        assert!(has_permission(&["tool:*".into()], "tool:mutate_data"));
        assert!(!has_permission(&["tool:*".into()], "app:crm:customer.read"));
    }

    #[test]
    fn has_permission_integration_wildcard() {
        assert!(has_permission(&["integration:gmail:*".into()], "integration:gmail:send"));
        assert!(!has_permission(&["integration:gmail:*".into()], "integration:slack:send"));
    }
}
