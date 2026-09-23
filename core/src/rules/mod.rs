//! Governed declarative rules (ADR 0010).
//!
//! Apps declare database rules — checks, partial-index predicates, index
//! expressions and field shorthands — in a closed language that this module
//! parses, type-checks and compiles to SQL itself. No manifest string ever
//! reaches the database: only SQL emitted from a validated syntax tree.

mod ast;
mod emit;
mod parser;
mod pattern;
mod typing;

#[cfg(test)]
mod tests;

use rootcx_types::{EntityContract, FieldContract, IndexColumn, IndexContract};
use sha2::{Digest, Sha256};

use crate::data_types::{FieldType, enum_check_expr};
use crate::manifest::{MAX_IDENT_BYTES, fit_ident, is_system_field, quote_ident};
use ast::Expr;
use typing::{Checker, Field, Fields, Ty};

/// Field keys this Core understands. Installation refuses any other key.
pub(crate) const FIELD_KEYS: &[&str] = &[
    "name", "type", "precision", "scale", "required", "default_value", "enum_values",
    "references", "is_primary_key", "on_delete", "sensitive", "owner",
];

/// Bumped whenever emission changes meaning, so every rule is recompiled.
const GRAMMAR_VERSION: &str = "r1";
pub(crate) const MAX_CHECKS: usize = 64;
pub(crate) const MAX_INDEXES: usize = 64;

/// Where a compiled check came from. Adoption of an object created by an older
/// Core compares the previous manifest's declaration with this one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Origin {
    Check { expr: String },
    Enum { field: String },
    Shorthand,
}

#[derive(Debug, Clone)]
pub(crate) struct CompiledCheck {
    pub(crate) name: String,
    pub(crate) sql: String,
    /// `r1-<hash>`: grammar version, canonical tree and field types.
    pub(crate) tag: String,
    /// The FNV tag an older Core wrote for the same declaration, if any.
    pub(crate) legacy_tag: Option<String>,
    pub(crate) origin: Origin,
}

#[derive(Debug, Clone)]
pub(crate) struct CompiledIndex {
    pub(crate) name: String,
    pub(crate) unique: bool,
    pub(crate) trigram: bool,
    method: &'static str,
    keys: Vec<String>,
    /// Key values without ordering, for duplicate preflight.
    pub(crate) key_values: Vec<String>,
    pub(crate) predicate: Option<String>,
    pub(crate) tag: String,
    pub(crate) legacy_tag: String,
    pub(crate) declared: IndexContract,
    /// Plain columns of a trigram index, for adoption by catalog structure.
    pub(crate) columns: Vec<String>,
}

impl CompiledIndex {
    pub(crate) fn create_sql(&self, schema: &str, table: &str, name: &str, concurrently: bool) -> String {
        let mut sql = format!(
            "CREATE {}INDEX {}{} ON {}.{} USING {} ({})",
            if self.unique { "UNIQUE " } else { "" },
            if concurrently { "CONCURRENTLY " } else { "" },
            quote_ident(name),
            quote_ident(schema),
            quote_ident(table),
            self.method,
            self.keys.join(", "),
        );
        if let Some(predicate) = &self.predicate {
            sql.push_str(&format!(" WHERE {predicate}"));
        }
        sql
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct EntityRules {
    pub(crate) checks: Vec<CompiledCheck>,
    pub(crate) indexes: Vec<CompiledIndex>,
}

/// The schema that owns `pg_trgm`, so no app schema holds an operator class
/// another app's index depends on.
pub(crate) const EXTENSION_SCHEMA: &str = "rootcx_ext";

/// Compile every rule an entity declares. Errors name the rule and position.
pub(crate) fn compile_entity(entity: &EntityContract) -> Result<EntityRules, String> {
    let fields = field_types(entity)?;
    let checker = Checker { fields: &fields };
    let mut rules = EntityRules::default();

    if entity.checks.len() > MAX_CHECKS {
        return Err(format!("entities are limited to {MAX_CHECKS} checks"));
    }
    if entity.indexes.len() > MAX_INDEXES {
        return Err(format!("entities are limited to {MAX_INDEXES} indexes"));
    }

    for field in &entity.fields {
        if let Some(check) = enum_check(entity, field)? {
            rules.checks.push(check);
        }
    }
    for (i, check) in entity.checks.iter().enumerate() {
        let label = check.name.clone().unwrap_or_else(|| i.to_string());
        let at = |message: String| format!("checks[{label}].expr {message}");
        let name = check.name.clone().ok_or_else(|| format!("checks[{i}]: every check needs a 'name'"))?;
        crate::manifest::validate_new_ident(&name, "check name").map_err(|e| e.to_string())?;
        let parsed = parser::parse(&check.expr).map_err(|e| at(e.to_string()))?;
        let resolved = checker.boolean(&parsed).map_err(|e| at(e))?;
        sensitive_scope(&resolved, &fields, false).map_err(|e| at(e))?;
        rules.checks.push(CompiledCheck {
            tag: tag("chk", &resolved, &fields),
            sql: emit::sql(&resolved),
            legacy_tag: Some(legacy_check_tag(&check.expr)),
            origin: Origin::Check { expr: check.expr.clone() },
            name,
        });
    }

    let mut names = std::collections::HashSet::new();
    for check in &rules.checks {
        debug_assert!(check.name.len() <= MAX_IDENT_BYTES, "generated names are fitted");
        if !names.insert(check.name.clone()) {
            return Err(format!("duplicate check name '{}'", check.name));
        }
    }

    let mut index_names = std::collections::HashSet::new();
    for (i, index) in entity.indexes.iter().enumerate() {
        let compiled = compile_index(entity, index, &fields, &checker)
            .map_err(|e| format!("indexes[{}] {e}", index.name.clone().unwrap_or_else(|| i.to_string())))?;
        if !index_names.insert(compiled.name.clone()) {
            return Err(format!("duplicate index name '{}'", compiled.name));
        }
        rules.indexes.push(compiled);
    }
    Ok(rules)
}

fn field_types(entity: &EntityContract) -> Result<Fields, String> {
    let mut fields = Fields::new();
    for field in &entity.fields {
        let ty = FieldType::from_field(field)?;
        fields.insert(field.name.clone(), Field { ty: Ty::of(&ty), sensitive: field.sensitive, pg: ty.postgres_type() });
    }
    for (name, ty) in [("id", FieldType::Uuid), ("created_at", FieldType::Timestamp), ("updated_at", FieldType::Timestamp)] {
        fields.entry(name.to_string()).or_insert(Field { ty: Ty::of(&ty), sensitive: false, pg: ty.postgres_type() });
    }
    Ok(fields)
}

/// A rule touching a sensitive field may touch no other field: once stored
/// values satisfy it, it reveals nothing beyond the rule itself. Index
/// predicates and expressions may not touch sensitive fields at all.
fn sensitive_scope(expr: &Expr, fields: &Fields, index: bool) -> Result<(), String> {
    let mut used = Vec::new();
    expr.fields(&mut used);
    if let Some(name) = used.iter().find(|name| fields.get(**name).is_some_and(|f| f.sensitive)) {
        if index {
            return Err(format!("field '{name}' is sensitive; index predicates and expressions cannot use it"));
        }
        if used.len() > 1 {
            return Err(format!("field '{name}' is sensitive; it can only appear in single-field rules"));
        }
    }
    Ok(())
}

fn tag(kind: &str, resolved: &Expr, fields: &Fields) -> String {
    let mut used = Vec::new();
    resolved.fields(&mut used);
    used.sort_unstable();
    let types: Vec<(&str, &str)> = used.iter().map(|name| (*name, fields[*name].pg.as_str())).collect();
    let canonical = serde_json::json!([GRAMMAR_VERSION, kind, resolved, types]);
    let digest = Sha256::digest(canonical.to_string().as_bytes());
    format!("{GRAMMAR_VERSION}-{}", hex::encode(&digest[..8]))
}

/// The tag an older Core wrote for a declared check: FNV-1a over the
/// whitespace-normalized expression string.
pub(crate) fn legacy_check_tag(expr: &str) -> String {
    fnv1a_hex(&expr.split_whitespace().collect::<Vec<_>>().join(" "))
}

pub(crate) fn fnv1a_hex(s: &str) -> String {
    let mut h = 0xcbf29ce484222325u64;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

fn enum_check(entity: &EntityContract, field: &FieldContract) -> Result<Option<CompiledCheck>, String> {
    let Some(values) = field.enum_values.as_ref().filter(|values| !values.is_empty()) else {
        return Ok(None);
    };
    // Built directly rather than parsed: PostgreSQL coerces the quoted values
    // to the column type, exactly as the legacy enum check did.
    let resolved = match FieldType::from_field(field)? {
        FieldType::TextArray => Expr::ArrayWithin { field: field.name.clone(), values: values.clone() },
        _ => Expr::In {
            e: Box::new(Expr::Field { name: field.name.clone() }),
            list: values.iter().map(|value| Expr::Text { value: value.clone() }).collect(),
            negated: false,
        },
    };
    let fields = field_types(entity)?;
    Ok(Some(CompiledCheck {
        name: fit_ident(&format!("chk_{}_{}", entity.entity_name, field.name)),
        sql: emit::sql(&resolved),
        tag: tag("chk", &resolved, &fields),
        legacy_tag: enum_check_expr(field).map(|expr| legacy_check_tag(&expr)),
        origin: Origin::Enum { field: field.name.clone() },
    }))
}

fn compile_index(
    entity: &EntityContract,
    index: &IndexContract,
    fields: &Fields,
    checker: &Checker,
) -> Result<CompiledIndex, String> {
    if index.columns.is_empty() {
        return Err("has no columns".into());
    }
    if !index.with.is_empty() {
        return Err("storage parameters ('with') are not allowed".into());
    }
    let using = index.using.as_deref().unwrap_or("btree").to_ascii_lowercase();
    let trigram = using == "trigram";
    let method: &'static str = match using.as_str() {
        "btree" => "btree",
        "hash" => "hash",
        "gist" => "gist",
        "gin" | "trigram" => "gin",
        "spgist" => "spgist",
        "brin" => "brin",
        other => return Err(format!("unknown index method '{other}'")),
    };
    if trigram && index.unique {
        return Err("a trigram index cannot be unique".into());
    }

    let mut keys = Vec::new();
    let mut key_values = Vec::new();
    let mut key_trees = Vec::new();
    let mut columns = Vec::new();
    for column in &index.columns {
        let (value, tree, sort, nulls) = match column {
            IndexColumn::Name(name) => (column_value(name, fields)?, serde_json::json!(name), None, None),
            IndexColumn::Spec(spec) => {
                if spec.ops.is_some() {
                    return Err("operator classes ('ops') are not allowed; use \"using\": \"trigram\" for trigram search".into());
                }
                let (value, tree) = match (&spec.column, &spec.expr) {
                    (Some(name), None) => (column_value(name, fields)?, serde_json::json!(name)),
                    (None, Some(expr)) => {
                        let parsed = parser::parse(expr).map_err(|e| format!("expr {e}"))?;
                        let resolved = checker.scalar(&parsed).map_err(|e| format!("expr {e}"))?;
                        sensitive_scope(&resolved, fields, true)?;
                        (format!("({})", emit::sql(&resolved)), serde_json::to_value(&resolved).expect("tree serializes"))
                    }
                    (Some(_), Some(_)) => return Err("a column has both 'column' and 'expr'".into()),
                    (None, None) => return Err("a column has neither 'column' nor 'expr'".into()),
                };
                (value, tree, spec.sort.as_deref(), spec.nulls.as_deref())
            }
        };
        let mut key = value.clone();
        if trigram {
            let (IndexColumn::Name(name) | IndexColumn::Spec(rootcx_types::IndexColumnSpec { column: Some(name), expr: None, sort: None, nulls: None, .. })) = column else {
                return Err("trigram indexes take plain text columns".into());
            };
            if fields.get(name).map(|f| f.ty) != Some(Ty::Text) {
                return Err(format!("trigram index column '{name}' must be text"));
            }
            columns.push(name.clone());
            key.push_str(&format!(" {EXTENSION_SCHEMA}.gin_trgm_ops"));
        }
        if let Some(sort) = sort {
            key.push_str(match sort.to_ascii_lowercase().as_str() {
                "asc" => " ASC",
                "desc" => " DESC",
                other => return Err(format!("invalid sort '{other}' (asc|desc)")),
            });
        }
        if let Some(nulls) = nulls {
            key.push_str(match nulls.to_ascii_lowercase().as_str() {
                "first" => " NULLS FIRST",
                "last" => " NULLS LAST",
                other => return Err(format!("invalid nulls '{other}' (first|last)")),
            });
        }
        key_trees.push(serde_json::json!([tree, sort.map(str::to_ascii_lowercase), nulls.map(str::to_ascii_lowercase)]));
        keys.push(key);
        key_values.push(value);
    }

    let predicate = match &index.where_clause {
        None => None,
        Some(source) => {
            let parsed = parser::parse(source).map_err(|e| format!("where {e}"))?;
            let resolved = checker.boolean(&parsed).map_err(|e| format!("where {e}"))?;
            sensitive_scope(&resolved, fields, true)?;
            Some(resolved)
        }
    };

    if let Some(name) = &index.name {
        crate::manifest::validate_new_ident(name, "index name").map_err(|e| e.to_string())?;
    }
    let name = crate::schema_sync::resolve_index_name(&entity.entity_name, index);
    let mut used = Vec::new();
    for column in &index.columns {
        match column {
            IndexColumn::Name(name) | IndexColumn::Spec(rootcx_types::IndexColumnSpec { column: Some(name), .. }) => used.push(name.clone()),
            IndexColumn::Spec(_) => {}
        }
    }
    if let Some(predicate) = &predicate {
        let mut names = Vec::new();
        predicate.fields(&mut names);
        used.extend(names.into_iter().map(str::to_string));
    }
    used.sort_unstable();
    used.dedup();
    let types: Vec<(&str, &str)> = used.iter().filter_map(|n| fields.get(n).map(|f| (n.as_str(), f.pg.as_str()))).collect();
    let canonical = serde_json::json!([GRAMMAR_VERSION, "idx", index.unique, using, key_trees, predicate, types]);
    let digest = Sha256::digest(canonical.to_string().as_bytes());

    Ok(CompiledIndex {
        name,
        unique: index.unique,
        trigram,
        method,
        keys,
        key_values,
        predicate: predicate.as_ref().map(emit::sql),
        tag: format!("{GRAMMAR_VERSION}-{}", hex::encode(&digest[..8])),
        legacy_tag: crate::schema_sync::index_spec_hash(index),
        declared: index.clone(),
        columns,
    })
}

fn column_value(name: &str, fields: &Fields) -> Result<String, String> {
    if !is_system_field(name) && !fields.contains_key(name) {
        return Err(format!("column '{name}' is not a declared or system field"));
    }
    Ok(quote_ident(name))
}
