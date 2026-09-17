//! Refuse pending app-supplied SQL migrations without executing or recording them.
//!
//! The legacy ledger is read only to allow redeployment of already-applied files.
//! New schema changes belong in the declarative manifest or in an independently
//! authorized, administrator-controlled database migration.

use std::collections::HashSet;
use std::path::Path;

use sqlx::PgPool;

use crate::manifest::quote_ident;

/// Check `<app_dir>/migrations`. Success always returns an empty list: this path
/// never executes SQL from an app or marks a file as applied.
pub async fn run(pool: &PgPool, schema: &str, app_dir: &Path) -> Result<Vec<String>, String> {
    crate::governance::row_access::inspect_schema(pool, schema)
        .await.map_err(|error| error.to_string())?;
    let entries = match std::fs::read_dir(app_dir.join("migrations")) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(error) => return Err(format!("read migrations dir: {error}")),
    };
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("read migration entry: {error}"))?;
        let path = entry.path();
        if path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("sql"))
        {
            files.push(
                entry
                    .file_name()
                    .into_string()
                    .map_err(|_| "migration filename must be valid UTF-8".to_string())?,
            );
        }
    }
    if files.is_empty() {
        return Ok(vec![]);
    }
    files.sort();

    let ledger = format!("{}.schema_migrations", quote_ident(schema));
    let exists: bool = sqlx::query_scalar("SELECT to_regclass($1) IS NOT NULL")
        .bind(&ledger)
        .fetch_one(pool)
        .await
        .map_err(|error| format!("check migration history: {error}"))?;
    let applied: HashSet<String> = if exists {
        sqlx::query_scalar(&format!("SELECT filename FROM {ledger}"))
            .fetch_all(pool)
            .await
            .map_err(|error| format!("read migration history: {error}"))?
            .into_iter()
            .collect()
    } else {
        HashSet::new()
    };
    files.retain(|name| !applied.contains(name));
    if files.is_empty() {
        return Ok(vec![]);
    }
    Err(format!(
        "app-supplied SQL migrations are disabled; pending files: {}. \
         Use the declarative manifest for schema changes, or have an administrator \
         perform an independently controlled database migration and remove the \
         pending files from the backend archive. No SQL was executed and no files \
         were marked applied.",
        files.join(", ")
    ))
}
