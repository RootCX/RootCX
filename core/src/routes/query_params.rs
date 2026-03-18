use std::collections::HashMap;

/// Column definition: name → SQL type. Used as whitelist + cast source.
pub type ColumnDefs = &'static [(&'static str, &'static str)];

/// Reserved params that are never treated as filters.
const RESERVED: &[&str] = &["limit", "offset", "before", "order_by", "order"];

pub struct ParsedQuery {
    pub conditions: Vec<String>,
    pub binds: Vec<String>,
    pub order_col: &'static str,
    pub order_dir: &'static str,
    pub limit: i64,
}

impl ParsedQuery {
    pub fn where_clause(&self) -> String {
        if self.conditions.is_empty() { String::new() }
        else { format!("WHERE {}", self.conditions.join(" AND ")) }
    }
}

/// Parse query params into SQL conditions, binds, sorting, and cursor.
///
/// Supports `field=val` (eq), `field__neq`, `field__contains` (ILIKE),
/// `field__gte`, `field__lte`. Aliases map alternate param names to columns
/// (e.g. `app_id` → `table_schema`).
pub fn parse(
    params: &HashMap<String, String>,
    cols: ColumnDefs,
    aliases: &[(&str, &str)],
    default_limit: i64,
    default_order_col: &str,
) -> ParsedQuery {
    let mut conditions = Vec::new();
    let mut binds: Vec<String> = Vec::new();

    let col_type = |name: &str| -> Option<&'static str> {
        cols.iter().find(|(k, _)| *k == name).map(|(_, t)| *t)
    };

    // Aliases (e.g. app_id → table_schema)
    for &(alias, col) in aliases {
        if let Some(v) = params.get(alias) {
            binds.push(v.clone());
            conditions.push(format!("{col} = ${}", binds.len()));
        }
    }

    // field__op filters
    for (key, val) in params {
        let (field, op) = key.rsplit_once("__").unwrap_or((key.as_str(), "eq"));
        if col_type(field).is_none()
            || RESERVED.contains(&key.as_str())
            || (op == "eq" && aliases.iter().any(|(a, _)| *a == key.as_str()))
        {
            continue;
        }
        let ct = col_type(field).unwrap();
        binds.push(val.clone());
        let i = binds.len();
        let cond = match op {
            "eq" => format!("{field} = ${i}::{ct}"),
            "neq" => format!("{field} != ${i}::{ct}"),
            "contains" => format!("{field} ILIKE '%' || ${i} || '%'"),
            "gte" => format!("{field} >= ${i}::{ct}"),
            "lte" => format!("{field} <= ${i}::{ct}"),
            _ => { binds.pop(); continue; }
        };
        conditions.push(cond);
    }

    if let Some(b) = params.get("before").and_then(|v| v.parse::<i64>().ok()) {
        conditions.push(format!("id < {b}"));
    }

    let limit = params.get("limit").and_then(|v| v.parse().ok()).unwrap_or(default_limit).clamp(1, 1000);

    let order_col = params.get("order_by")
        .and_then(|c| col_type(c).map(|_| cols.iter().find(|(k, _)| *k == c).unwrap().0))
        .unwrap_or_else(|| cols.iter().find(|(k, _)| *k == default_order_col).map(|(k, _)| *k).unwrap_or("id"));

    let order_dir = match params.get("order").map(|s| s.as_str()) {
        Some("asc") => "ASC",
        _ => "DESC",
    };

    ParsedQuery { conditions, binds, order_col, order_dir, limit }
}
