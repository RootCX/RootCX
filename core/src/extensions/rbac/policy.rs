use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

use sqlx::PgPool;
use uuid::Uuid;

use crate::api_error::ApiError;
use rootcx_shared_types::PermissionsContract;

#[derive(Debug, Clone)]
pub struct CachedPolicy {
    pub role: String,
    pub entity: String,
    pub actions: Vec<String>,
    pub ownership: bool,
}

#[derive(Debug, Clone)]
pub struct CachedApp {
    pub roles: HashMap<String, Vec<String>>,
    pub policies: Vec<CachedPolicy>,
    pub default_role: Option<String>,
}

#[derive(Debug, Default)]
pub struct PolicyCache {
    inner: RwLock<HashMap<String, CachedApp>>,
}

impl PolicyCache {
    pub fn invalidate(&self, app_id: &str) {
        self.inner.write().unwrap().remove(app_id);
    }

    pub fn get(&self, app_id: &str) -> Option<CachedApp> {
        self.inner.read().unwrap().get(app_id).cloned()
    }

    pub fn populate(&self, app_id: &str, contract: &PermissionsContract) {
        self.inner.write().unwrap().insert(
            app_id.to_string(),
            CachedApp {
                roles: contract.roles.iter().map(|(k, v)| (k.clone(), v.inherits.clone())).collect(),
                policies: contract
                    .policies
                    .iter()
                    .map(|p| CachedPolicy {
                        role: p.role.clone(),
                        entity: p.entity.clone(),
                        actions: p.actions.clone(),
                        ownership: p.ownership,
                    })
                    .collect(),
                default_role: contract.default_role.clone(),
            },
        );
    }

    /// Get from cache, or load from DB on miss. Returns None if app has no RBAC.
    pub async fn get_or_fetch(&self, pool: &PgPool, app_id: &str) -> Result<Option<CachedApp>, ApiError> {
        if let Some(cached) = self.get(app_id) {
            return Ok(Some(cached));
        }

        let role_rows: Vec<(String, Vec<String>)> =
            sqlx::query_as("SELECT name, inherits FROM rootcx_system.rbac_roles WHERE app_id = $1")
                .bind(app_id)
                .fetch_all(pool)
                .await?;

        if role_rows.is_empty() {
            return Ok(None);
        }

        let policy_rows: Vec<(String, String, Vec<String>, bool)> = sqlx::query_as(
            "SELECT role, entity, actions, ownership FROM rootcx_system.rbac_policies WHERE app_id = $1",
        )
        .bind(app_id)
        .fetch_all(pool)
        .await?;

        let default_role: Option<String> =
            sqlx::query_scalar("SELECT manifest->'permissions'->>'defaultRole' FROM rootcx_system.apps WHERE id = $1")
                .bind(app_id)
                .fetch_optional(pool)
                .await?
                .flatten();

        let cached = CachedApp {
            roles: role_rows.into_iter().collect(),
            policies: policy_rows
                .into_iter()
                .map(|(role, entity, actions, ownership)| CachedPolicy { role, entity, actions, ownership })
                .collect(),
            default_role,
        };

        self.inner.write().unwrap().insert(app_id.to_string(), cached.clone());
        Ok(Some(cached))
    }
}

pub fn expand_roles(assigned: &[String], role_map: &HashMap<String, Vec<String>>) -> HashSet<String> {
    let mut expanded = HashSet::new();
    let mut stack: Vec<&str> = assigned.iter().map(|s| s.as_str()).collect();
    while let Some(role) = stack.pop() {
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

/// Evaluate policies: returns (allowed, ownership_required).
///
/// Ownership is only required when **all** matching policies demand it.
/// If any policy grants the action without an ownership constraint, the
/// user gets unrestricted access (the broader grant wins).
pub fn evaluate(
    expanded_roles: &HashSet<String>,
    entity: &str,
    action: &str,
    policies: &[CachedPolicy],
) -> (bool, bool) {
    let mut allowed = false;
    let mut has_unrestricted = false;
    for p in policies {
        if !expanded_roles.contains(&p.role) {
            continue;
        }
        if p.entity != "*" && p.entity != entity {
            continue;
        }
        let action_match = p.actions.iter().any(|a| a == "*" || a == action);
        if action_match {
            allowed = true;
            if !p.ownership {
                has_unrestricted = true;
            }
        }
    }
    (allowed, allowed && !has_unrestricted)
}

/// Resolve a user's expanded roles for an app (from DB assignments + default + hierarchy).
pub async fn resolve_user_roles(
    pool: &PgPool,
    cached: &CachedApp,
    user_id: Uuid,
    app_id: &str,
) -> Result<HashSet<String>, ApiError> {
    let assigned: Vec<(String,)> =
        sqlx::query_as("SELECT role FROM rootcx_system.rbac_assignments WHERE user_id = $1 AND app_id = $2")
            .bind(user_id)
            .bind(app_id)
            .fetch_all(pool)
            .await?;

    let mut roles: Vec<String> = assigned.into_iter().map(|(r,)| r).collect();
    if roles.is_empty()
        && let Some(ref default) = cached.default_role {
            roles.push(default.clone());
        }
    Ok(expand_roles(&roles, &cached.roles))
}

/// Check if user has admin-level access (entity:"*", actions containing "*").
pub async fn require_admin(
    pool: &PgPool,
    cache: &Arc<PolicyCache>,
    app_id: &str,
    user_id: Uuid,
) -> Result<(), ApiError> {
    let cached =
        cache.get_or_fetch(pool, app_id).await?.ok_or_else(|| ApiError::Forbidden("no RBAC configured".into()))?;
    let expanded = resolve_user_roles(pool, &cached, user_id, app_id).await?;
    let is_admin = cached
        .policies
        .iter()
        .any(|p| p.entity == "*" && expanded.contains(&p.role) && p.actions.iter().any(|a| a == "*"));
    if !is_admin {
        return Err(ApiError::Forbidden("admin access required".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roles(entries: &[(&str, &[&str])]) -> HashMap<String, Vec<String>> {
        entries.iter().map(|(k, v)| (k.to_string(), v.iter().map(|s| s.to_string()).collect())).collect()
    }

    fn policy(role: &str, entity: &str, actions: &[&str], ownership: bool) -> CachedPolicy {
        CachedPolicy {
            role: role.into(),
            entity: entity.into(),
            actions: actions.iter().map(|s| s.to_string()).collect(),
            ownership,
        }
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
        let expanded = expand_roles(&["admin".into()], &r);
        assert_eq!(expanded.len(), 3);
    }

    #[test]
    fn detect_cycle_none() {
        assert!(detect_cycle(&roles(&[("a", &["b"]), ("b", &["c"]), ("c", &[])])).is_none());
    }

    #[test]
    fn detect_cycle_direct() {
        assert!(detect_cycle(&roles(&[("a", &["b"]), ("b", &["a"])])).is_some());
    }

    #[test]
    fn detect_cycle_indirect() {
        assert!(detect_cycle(&roles(&[("a", &["b"]), ("b", &["c"]), ("c", &["a"])])).is_some());
    }

    #[test]
    fn evaluate_wildcard_entity() {
        let p = vec![policy("admin", "*", &["*"], false)];
        let (allowed, ownership) = evaluate(&["admin".into()].into(), "deals", "delete", &p);
        assert!(allowed && !ownership);
    }

    #[test]
    fn evaluate_specific_entity() {
        let p = vec![policy("rep", "deals", &["create", "read"], true)];
        let r: HashSet<String> = ["rep".into()].into();
        assert!(evaluate(&r, "deals", "create", &p) == (true, true));
        assert!(!evaluate(&r, "deals", "delete", &p).0);
        assert!(!evaluate(&r, "contacts", "read", &p).0);
    }

    #[test]
    fn evaluate_denied_role() {
        let p = vec![policy("admin", "*", &["*"], false)];
        assert!(!evaluate(&["viewer".into()].into(), "deals", "read", &p).0);
    }

    #[test]
    fn evaluate_union_semantics() {
        let p = vec![policy("viewer", "*", &["read"], false), policy("rep", "deals", &["create", "update"], true)];
        let r: HashSet<String> = ["viewer".into(), "rep".into()].into();
        assert_eq!(evaluate(&r, "deals", "read", &p), (true, false));
        assert_eq!(evaluate(&r, "deals", "create", &p), (true, true));
        assert_eq!(evaluate(&r, "contacts", "read", &p), (true, false));
        assert_eq!(evaluate(&r, "contacts", "create", &p), (false, false));
    }

    #[test]
    fn expand_roles_empty_assigned() {
        let r = roles(&[("admin", &[]), ("viewer", &[])]);
        assert!(expand_roles(&[], &r).is_empty());
    }

    #[test]
    fn expand_roles_unknown_role_ignored() {
        let r = roles(&[("admin", &[])]);
        let expanded = expand_roles(&["ghost".into()], &r);
        // Unknown role still appears (it's assigned) but doesn't pull others
        assert_eq!(expanded.len(), 1);
        assert!(expanded.contains("ghost"));
    }

    #[test]
    fn detect_cycle_self_referencing() {
        assert!(detect_cycle(&roles(&[("a", &["a"])])).is_some());
    }

    #[test]
    fn evaluate_unrestricted_overrides_ownership() {
        // viewer grants read/* without ownership, rep grants read/deals with ownership.
        // The broader grant (viewer) lifts the ownership restriction.
        let p = vec![policy("viewer", "deals", &["read"], false), policy("rep", "deals", &["read"], true)];
        let r: HashSet<String> = ["viewer".into(), "rep".into()].into();
        assert_eq!(evaluate(&r, "deals", "read", &p), (true, false));
    }

    #[test]
    fn evaluate_all_ownership_policies() {
        // When ALL matching policies require ownership, it stays required.
        let p = vec![policy("rep", "deals", &["read", "update"], true), policy("support", "deals", &["read"], true)];
        let r: HashSet<String> = ["rep".into(), "support".into()].into();
        assert_eq!(evaluate(&r, "deals", "read", &p), (true, true));
        assert_eq!(evaluate(&r, "deals", "update", &p), (true, true));
    }

    #[test]
    fn evaluate_no_policies() {
        let (allowed, _) = evaluate(&["admin".into()].into(), "deals", "read", &[]);
        assert!(!allowed);
    }

    #[test]
    fn evaluate_wildcard_action() {
        let p = vec![policy("editor", "deals", &["*"], false)];
        assert!(evaluate(&["editor".into()].into(), "deals", "delete", &p).0);
    }

    #[test]
    fn expand_roles_diamond_hierarchy() {
        // admin → editor, admin → reviewer, both → viewer
        let r = roles(&[
            ("admin", &["editor", "reviewer"]),
            ("editor", &["viewer"]),
            ("reviewer", &["viewer"]),
            ("viewer", &[]),
        ]);
        let expanded = expand_roles(&["admin".into()], &r);
        assert_eq!(expanded.len(), 4);
    }

    #[test]
    fn detect_cycle_disconnected_graph() {
        // Two disconnected subgraphs, neither has a cycle
        let r = roles(&[("a", &["b"]), ("b", &[]), ("x", &["y"]), ("y", &[])]);
        assert!(detect_cycle(&r).is_none());
    }

    #[test]
    fn default_role_feeds_into_evaluate() {
        // Simulates the resolve_user_roles path: no assignments → default_role → expand → evaluate
        let role_map = roles(&[("member", &["viewer"]), ("viewer", &[])]);
        let policies = vec![policy("viewer", "*", &["read"], false), policy("member", "deals", &["create"], true)];
        let default_role = "member";
        let expanded = expand_roles(&[default_role.into()], &role_map);
        assert!(expanded.contains("member") && expanded.contains("viewer"));
        // Viewer grants unrestricted read everywhere
        assert_eq!(evaluate(&expanded, "deals", "read", &policies), (true, false));
        // Member grants create on deals with ownership (only matching policy)
        assert_eq!(evaluate(&expanded, "deals", "create", &policies), (true, true));
        // No policy grants delete
        assert_eq!(evaluate(&expanded, "deals", "delete", &policies), (false, false));
    }
}
