use std::collections::{HashMap, HashSet};

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

/// The weakest key satisfying both grants, or `None` when they are disjoint.
///
/// Gating and narrowing ask different questions. `has_permission` answers a
/// partial order — "does the holder satisfy this requirement?" — and stays exact:
/// it also backs the anti-escalation subset check in `delegation::act_as`, which
/// must not accept a weaker key as equivalent. This answers a lattice meet —
/// "what is the weaker of these two?" — and is used only where authority is
/// narrowed. Scope: `All ⊐ Own`, encoded as the `.own` suffix, which
/// `manifest::validate_perm_key` reserves for core-minted keys so the relation is
/// a fact about provenance rather than a naming convention.
pub(crate) fn meet(key: &str, other: &[String]) -> Option<String> {
    if has_permission(other, key) {
        return Some(key.to_string());
    }
    // Descend only: an `All` grant narrows to `Own`, never the reverse — an
    // already-scoped key has nothing weaker to fall back to.
    if key.ends_with(".own") {
        return None;
    }
    let scoped = format!("{key}.own");
    has_permission(other, &scoped).then_some(scoped)
}

/// Authority narrowed to what both sides allow. A delegated agent runs with this
/// set, so it must never exceed either grant — where the two disagree on scope,
/// the weaker scope wins rather than the pair cancelling out.
pub fn intersect_permissions(a: &[String], b: &[String]) -> Vec<String> {
    if a.iter().any(|p| p == "*") { return b.to_vec(); }
    if b.iter().any(|p| p == "*") { return a.to_vec(); }
    let mut result: Vec<String> = a.iter().filter_map(|p| meet(p, b)).collect();
    result.extend(b.iter().filter_map(|p| meet(p, a)));
    result.sort_unstable();
    result.dedup();
    result
}

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
        assert_eq!(intersect_permissions(&a, &b), vec!["app:crm:customer.read", "tool:query_data"]);
    }
    #[test]
    fn intersect_no_overlap() {
        assert!(intersect_permissions(&["app:crm:x".into()], &["app:support:y".into()]).is_empty());
    }
    #[test]
    fn intersect_global_wildcard() {
        let b = vec!["app:crm:customer.read".into(), "tool:query_data".into()];
        assert_eq!(intersect_permissions(&["*".into()], &b), b);
    }
    #[test]
    fn intersect_both_wildcard() {
        assert_eq!(intersect_permissions(&["*".into()], &["*".into()]), vec!["*"]);
    }
    #[test]
    fn intersect_scoped_wildcard() {
        let a = vec!["app:crm:*".into()];
        let b = vec!["app:crm:customer.read".into(), "app:support:ticket.read".into()];
        assert_eq!(intersect_permissions(&a, &b), vec!["app:crm:customer.read"]);
    }
    #[test]
    fn intersect_empty() {
        let empty: Vec<String> = vec![];
        assert!(intersect_permissions(&empty, &empty).is_empty());
        assert!(intersect_permissions(&["x".into()], &empty).is_empty());
    }
    /// The scope lattice: where two grants disagree on scope, narrowing must
    /// yield the WEAKER key rather than cancelling out. Before this existed the
    /// pair below intersected to nothing, which denied the delegated agent
    /// everything. The direction matters as much as the non-emptiness: emitting
    /// the base key instead would widen the agent past its own grant.
    #[test]
    fn intersect_narrows_to_the_weaker_scope() {
        let cases: Vec<(&[&str], &[&str], Vec<&str>, &str)> = vec![
            (
                &["app:crm:c.read"],
                &["app:crm:c.read.own"],
                vec!["app:crm:c.read.own"],
                "agent sees all, human only their own -> own",
            ),
            (
                &["app:crm:c.read.own"],
                &["app:crm:c.read"],
                vec!["app:crm:c.read.own"],
                "symmetric: argument order must not change the result",
            ),
            (
                &["app:crm:c.read.own"],
                &["app:crm:c.read.own"],
                vec!["app:crm:c.read.own"],
                "both scoped -> unchanged",
            ),
            (
                &["app:crm:*"],
                &["app:crm:c.read.own"],
                vec!["app:crm:c.read.own"],
                "an app wildcard already covers the scoped key",
            ),
            (
                &["app:crm:c.read"],
                &["app:crm:other.read.own"],
                vec![],
                "different entities stay disjoint",
            ),
            (
                &["app:crm:c.read.own"],
                &["app:crm:c.write"],
                vec![],
                "different actions stay disjoint",
            ),
        ];
        for (a, b, expected, why) in cases {
            let a: Vec<String> = a.iter().map(|s| s.to_string()).collect();
            let b: Vec<String> = b.iter().map(|s| s.to_string()).collect();
            assert_eq!(intersect_permissions(&a, &b), expected, "{why}");
        }
    }

    #[test]
    fn has_permission_exact() {
        assert!(has_permission(&["app:crm:customer.read".into()], "app:crm:customer.read"));
        assert!(!has_permission(&["app:crm:customer.read".into()], "app:crm:customer.write"));
    }
    /// Regression guard for the one refactor this design forbids: moving the
    /// scope lattice into `has_permission`. That would silently widen every gate
    /// built on it — including the anti-escalation subset check in
    /// `delegation::act_as`, which must never accept a weaker key as equivalent.
    /// `meet` is where the relation lives; here it must stay invisible.
    #[test]
    fn has_permission_ignores_the_scope_lattice() {
        for (held, required, why) in [
            ("app:crm:c.read", "app:crm:c.read.own", "base must not satisfy scoped"),
            ("app:crm:c.read.own", "app:crm:c.read", "scoped must not satisfy base"),
        ] {
            assert!(
                !has_permission(&[held.to_string()], required),
                "{why} (held={held}, required={required})"
            );
        }
    }

    #[test]
    fn has_permission_wildcards() {
        assert!(has_permission(&["*".into()], "anything"));
        assert!(has_permission(&["app:crm:*".into()], "app:crm:customer.read"));
        assert!(!has_permission(&["app:crm:*".into()], "app:support:x"));
        assert!(has_permission(&["tool:*".into()], "tool:query_data"));
        assert!(!has_permission(&["tool:*".into()], "app:x:y"));
    }
}
