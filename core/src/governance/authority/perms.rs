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
    #[test]
    fn has_permission_exact() {
        assert!(has_permission(&["app:crm:customer.read".into()], "app:crm:customer.read"));
        assert!(!has_permission(&["app:crm:customer.read".into()], "app:crm:customer.write"));
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
