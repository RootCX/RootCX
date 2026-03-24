use std::path::Path;

use sqlx::PgPool;
use tracing::info;

use crate::RuntimeError;
use crate::manifest::quote_ident;

pub(crate) fn list_pending_files(migrations_dir: &Path) -> Result<Vec<(String, String)>, RuntimeError> {
    let mut entries: Vec<(String, String)> = Vec::new();

    let read = std::fs::read_dir(migrations_dir)
        .map_err(|e| RuntimeError::Migration(format!("read migrations dir: {e}")))?;

    for entry in read {
        let entry = entry.map_err(|e| RuntimeError::Migration(format!("read entry: {e}")))?;
        let path = entry.path();

        if path.extension().and_then(|e| e.to_str()) != Some("sql") {
            continue;
        }

        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        validate_filename(&name)?;

        let sql = std::fs::read_to_string(&path)
            .map_err(|e| RuntimeError::Migration(format!("read {name}: {e}")))?;
        if sql.trim().is_empty() {
            return Err(RuntimeError::Migration(format!("empty migration: {name}")));
        }

        entries.push((name, sql));
    }

    entries.sort_by(|a, b| a.0.cmp(&b.0));
    check_duplicate_prefixes(&entries)?;
    Ok(entries)
}

fn validate_filename(name: &str) -> Result<(), RuntimeError> {
    let prefix = name.split('_').next().unwrap_or("");
    let stem = name.strip_suffix(".sql").unwrap_or(name);
    if prefix.is_empty()
        || !prefix.bytes().all(|b| b.is_ascii_digit())
        || stem.len() == prefix.len()
    {
        return Err(RuntimeError::Migration(format!(
            "invalid migration filename '{name}': expected NNN_description.sql"
        )));
    }
    Ok(())
}

fn check_duplicate_prefixes(entries: &[(String, String)]) -> Result<(), RuntimeError> {
    for pair in entries.windows(2) {
        let pa = pair[0].0.split('_').next().unwrap_or("");
        let pb = pair[1].0.split('_').next().unwrap_or("");
        if pa == pb {
            return Err(RuntimeError::Migration(format!(
                "duplicate migration prefix '{pa}': {} and {}",
                pair[0].0, pair[1].0
            )));
        }
    }
    Ok(())
}

fn split_statements(sql: &str) -> Vec<&str> {
    let mut stmts = Vec::new();
    let mut start = 0;
    let mut chars = sql.char_indices().peekable();
    while let Some((i, ch)) = chars.next() {
        match ch {
            '\'' => {
                while let Some((_, c)) = chars.next() {
                    if c == '\'' {
                        if chars.peek().is_some_and(|(_, nc)| *nc == '\'') {
                            chars.next();
                        } else {
                            break;
                        }
                    }
                }
            }
            '$' => {
                if let Some(tag_end) = find_dollar_quote(sql, i) {
                    let tag = &sql[i..=tag_end];
                    let skip = tag.len() - 1;
                    for _ in 0..skip { chars.next(); }
                    while let Some((j, _)) = chars.next() {
                        if sql[j..].starts_with(tag) {
                            for _ in 0..skip { chars.next(); }
                            break;
                        }
                    }
                }
            }
            ';' => {
                let stmt = sql[start..i].trim();
                if !stmt.is_empty() { stmts.push(stmt); }
                start = i + 1;
            }
            _ => {}
        }
    }
    let tail = sql[start..].trim();
    if !tail.is_empty() { stmts.push(tail); }
    stmts
}

fn find_dollar_quote(sql: &str, start: usize) -> Option<usize> {
    for (i, ch) in sql[start + 1..].char_indices() {
        if ch == '$' { return Some(start + 1 + i); }
        if !ch.is_ascii_alphanumeric() && ch != '_' { return None; }
    }
    None
}

pub(crate) async fn ensure_migrations_table(pool: &PgPool, app_schema: &str) -> Result<(), RuntimeError> {
    let sql = format!(
        "CREATE TABLE IF NOT EXISTS {}._migrations (\
            name TEXT PRIMARY KEY, \
            applied_at TIMESTAMPTZ NOT NULL DEFAULT now()\
        )",
        quote_ident(app_schema)
    );
    sqlx::query(&sql).execute(pool).await.map_err(RuntimeError::Schema)?;
    Ok(())
}

pub(crate) async fn list_applied(pool: &PgPool, app_schema: &str) -> Result<Vec<String>, RuntimeError> {
    let sql = format!(
        "SELECT name FROM {}._migrations ORDER BY name",
        quote_ident(app_schema)
    );
    let rows: Vec<(String,)> = sqlx::query_as(&sql)
        .fetch_all(pool)
        .await
        .map_err(RuntimeError::Schema)?;
    Ok(rows.into_iter().map(|(n,)| n).collect())
}

pub(crate) async fn run_pending(
    pool: &PgPool,
    app_schema: &str,
    app_dir: &Path,
) -> Result<usize, RuntimeError> {
    let migrations_dir = app_dir.join("migrations");
    if !migrations_dir.is_dir() {
        return Ok(0);
    }

    let all_files = list_pending_files(&migrations_dir)?;
    if all_files.is_empty() {
        return Ok(0);
    }

    let create_schema = format!("CREATE SCHEMA IF NOT EXISTS {}", quote_ident(app_schema));
    sqlx::query(&create_schema).execute(pool).await.map_err(RuntimeError::Schema)?;

    ensure_migrations_table(pool, app_schema).await?;
    let applied: std::collections::HashSet<String> = list_applied(pool, app_schema).await?.into_iter().collect();

    let pending: Vec<_> = all_files
        .into_iter()
        .filter(|(name, _)| !applied.contains(name))
        .collect();

    if pending.is_empty() {
        return Ok(0);
    }

    let mut count = 0;
    for (name, sql) in &pending {
        let mut tx = pool.begin().await.map_err(RuntimeError::Schema)?;

        for stmt in split_statements(sql) {
            sqlx::query(stmt)
                .execute(&mut *tx)
                .await
                .map_err(|e| RuntimeError::Migration(format!("{name}: {e}")))?;
        }

        let record = format!(
            "INSERT INTO {}._migrations (name) VALUES ($1)",
            quote_ident(app_schema)
        );
        sqlx::query(&record)
            .bind(name)
            .execute(&mut *tx)
            .await
            .map_err(RuntimeError::Schema)?;

        tx.commit().await.map_err(RuntimeError::Schema)?;
        count += 1;
        info!(app = %app_schema, migration = %name, "applied");
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_migration(dir: &Path, name: &str, content: &str) {
        let migrations = dir.join("migrations");
        std::fs::create_dir_all(&migrations).unwrap();
        std::fs::write(migrations.join(name), content).unwrap();
    }

    #[test]
    fn list_pending_sorts_lexicographically() {
        let dir = TempDir::new().unwrap();
        write_migration(dir.path(), "003_third.sql", "SELECT 3");
        write_migration(dir.path(), "001_first.sql", "SELECT 1");
        write_migration(dir.path(), "002_second.sql", "SELECT 2");

        let files = list_pending_files(&dir.path().join("migrations")).unwrap();
        let names: Vec<_> = files.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["001_first.sql", "002_second.sql", "003_third.sql"]);
    }

    #[test]
    fn list_pending_rejects_duplicate_prefix() {
        let dir = TempDir::new().unwrap();
        write_migration(dir.path(), "001_a.sql", "SELECT 1");
        write_migration(dir.path(), "001_b.sql", "SELECT 2");

        let err = list_pending_files(&dir.path().join("migrations")).unwrap_err();
        assert!(err.to_string().contains("duplicate migration prefix"), "{err}");
    }

    #[test]
    fn list_pending_ignores_non_sql() {
        let dir = TempDir::new().unwrap();
        write_migration(dir.path(), "001_init.sql", "SELECT 1");
        write_migration(dir.path(), "README.md", "# readme");
        write_migration(dir.path(), ".gitkeep", "");

        let files = list_pending_files(&dir.path().join("migrations")).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].0, "001_init.sql");
    }

    #[test]
    fn list_pending_empty_dir() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("migrations")).unwrap();

        let files = list_pending_files(&dir.path().join("migrations")).unwrap();
        assert!(files.is_empty());
    }

    #[test]
    fn list_pending_rejects_bad_filenames() {
        for (name, label) in [
            ("foo.sql", "no numeric prefix"),
            ("001.sql", "no description after prefix"),
        ] {
            let dir = TempDir::new().unwrap();
            write_migration(dir.path(), name, "SELECT 1");

            let err = list_pending_files(&dir.path().join("migrations")).unwrap_err();
            assert!(err.to_string().contains("invalid migration filename"), "{label}: {err}");
        }
    }

    #[test]
    fn list_pending_rejects_empty_sql() {
        let dir = TempDir::new().unwrap();
        write_migration(dir.path(), "001_empty.sql", "   \n  ");

        let err = list_pending_files(&dir.path().join("migrations")).unwrap_err();
        assert!(err.to_string().contains("empty migration"), "{err}");
    }

    #[test]
    fn split_statements_basic() {
        let stmts = split_statements("CREATE TABLE a (id INT); CREATE TABLE b (id INT)");
        assert_eq!(stmts, ["CREATE TABLE a (id INT)", "CREATE TABLE b (id INT)"]);
    }

    #[test]
    fn split_statements_respects_strings_and_dollar_quotes() {
        for (label, sql, expected_count, must_contain) in [
            ("simple string", "INSERT INTO a VALUES ('hello; world'); SELECT 1", 2, "hello; world"),
            ("escaped quotes", "INSERT INTO a VALUES ('it''s a ;test'); SELECT 1", 2, "it''s a ;test"),
            ("anonymous dollar", "DO $$ BEGIN NULL; END $$; SELECT 1", 2, "NULL;"),
            ("named dollar tag", "CREATE FUNCTION $fn$ BEGIN; RETURN; END $fn$; SELECT 1", 2, "RETURN;"),
        ] {
            let stmts = split_statements(sql);
            assert_eq!(stmts.len(), expected_count, "{label}: {stmts:?}");
            assert!(stmts[0].contains(must_contain), "{label}: {stmts:?}");
        }
    }

    #[test]
    fn split_statements_handles_utf8() {
        let sql = "-- ── Schema ─────\nCREATE TABLE a (id INT); SELECT 1";
        let stmts = split_statements(sql);
        assert_eq!(stmts.len(), 2, "{stmts:?}");
        assert!(stmts[0].contains("CREATE TABLE"), "{stmts:?}");
    }

    #[test]
    fn split_statements_empty_and_whitespace() {
        assert!(split_statements("").is_empty());
        assert!(split_statements("  \n  ").is_empty());
        let stmts = split_statements("SELECT 1;  ;  ; SELECT 2");
        assert_eq!(stmts, ["SELECT 1", "SELECT 2"]);
    }

}
