use std::collections::{HashMap, HashSet};

use sqlx::PgPool;
use uuid::Uuid;

use crate::api_error::ApiError;

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
