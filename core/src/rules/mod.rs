//! Governed declarative rules (ADR 0010).
//!
//! Apps declare database rules — checks, partial-index predicates, index
//! expressions and field shorthands — in a closed language that this module
//! parses, type-checks and compiles to SQL itself. No manifest string ever
//! reaches the database: only SQL emitted from a validated syntax tree.

/// Field keys this Core understands. Installation refuses any other key.
pub(crate) const FIELD_KEYS: &[&str] = &[
    "name", "type", "precision", "scale", "required", "default_value", "enum_values",
    "references", "is_primary_key", "on_delete", "sensitive", "owner",
];
