use rootcx_types::{AppManifest, EntityContract};
use sqlx::PgConnection;

use crate::RuntimeError;
use crate::manifest::{RefTarget, parse_entity_ref, quote_ident, quote_literal};

use super::{
    MAX_OWNER_CHAIN, OwnerMap, exec, fits_ident_limit, owner_field, owner_parent,
    policies, shared_entities,
};

fn invalid(message: impl Into<String>) -> RuntimeError {
    RuntimeError::Invalid(message.into())
}

fn entity<'a>(manifest: &'a AppManifest, name: &str) -> Result<&'a EntityContract, RuntimeError> {
    manifest.data_contract.iter().find(|e| e.entity_name == name)
        .ok_or_else(|| invalid(format!("share: unknown entity '{name}'")))
}

fn primary(entity: &EntityContract) -> &str {
    entity.fields.iter().find(|f| f.is_primary_key == Some(true) || f.name == "id")
        .map(|f| f.name.as_str()).unwrap_or("id")
}

fn link<'a>(manifest: &'a AppManifest, source: &EntityContract, field: &str) -> Result<&'a EntityContract, RuntimeError> {
    let field = source.fields.iter().find(|f| f.name == field)
        .ok_or_else(|| invalid(format!("entity '{}': share field '{field}' does not exist", source.entity_name)))?;
    let reference = field.references.as_ref().filter(|_| field.field_type == "entity_link")
        .ok_or_else(|| invalid(format!("entity '{}': share field '{}' must be a local entity_link", source.entity_name, field.name)))?;
    if !matches!(parse_entity_ref(&reference.entity), RefTarget::Local(_)) {
        return Err(invalid(format!("entity '{}': share field '{}' must link to a local entity", source.entity_name, field.name)));
    }
    let target = entity(manifest, &reference.entity)?;
    if reference.field != primary(target) {
        return Err(invalid(format!("entity '{}': share field '{}' must reference the primary key of '{}'", source.entity_name, field.name, target.entity_name)));
    }
    if owner_field(target).is_none() {
        return Err(invalid(format!("entity '{}': share field '{}' targets '{}', which declares no owner field", source.entity_name, field.name, target.entity_name)));
    }
    Ok(target)
}

pub(super) fn validate(manifest: &AppManifest) -> Result<(), RuntimeError> {
    for source in &manifest.data_contract {
        let Some(share) = &source.share else { continue };
        let index = format!("rootcx_share_{}_{}", manifest.app_id, source.entity_name);
        if !fits_ident_limit(&index) {
            return Err(invalid(format!("share index '{index}' exceeds PostgreSQL's 63-byte identifier limit")));
        }
        if share.grantee == share.subject {
            return Err(invalid(format!("entity '{}': share grantee and subject must be different fields", source.entity_name)));
        }
        link(manifest, source, &share.grantee)?;
        link(manifest, source, &share.subject)?;
        let active = source.fields.iter().find(|f| f.name == share.active_when.is_null)
            .ok_or_else(|| invalid(format!("entity '{}': share activeWhen.isNull names missing field '{}'", source.entity_name, share.active_when.is_null)))?;
        if !matches!(active.field_type.as_str(), "date" | "timestamp") || active.required {
            return Err(invalid(format!("entity '{}': share activeWhen.isNull field '{}' must be a nullable date or timestamp", source.entity_name, active.name)));
        }
    }
    for owned in shared_entities(manifest) {
        // Resolvers are callable by the executor and return ownership keys.
        // Column SELECT grants cannot hide values returned by a definer function.
        if owned.fields.iter().any(|field| field.owner && field.sensitive) {
            return Err(invalid(format!(
                "entity '{}': shared access requires a nonsensitive owner field; its resolver returns ownership keys",
                owned.entity_name,
            )));
        }
        let resolver = format!("rootcx_shared.{}.{}", manifest.app_id, owned.entity_name);
        if !fits_ident_limit(&resolver) {
            return Err(invalid(format!("share resolver '{resolver}' exceeds PostgreSQL's 63-byte identifier limit")));
        }
        // Follow the same ownership graph; sharing never recursively follows
        // another share and never imports another grantee's authority.
        path(manifest, &owned.entity_name, "v")?;
    }
    Ok(())
}

struct Path {
    from: String,
    identity: String,
    identity_type: &'static str,
    pk: String,
}

/// Resolve a declared ownership path into joins. No caller-supplied expressions
/// or casts of indexed columns; the user/set is converted to the column's type.
fn path(manifest: &AppManifest, start: &str, prefix: &str) -> Result<Path, RuntimeError> {
    let mut current = entity(manifest, start)?;
    let mut visited = Vec::new();
    let mut from = format!("{}.{} {prefix}0", quote_ident(&manifest.app_id), quote_ident(start));
    let pk = format!("{prefix}0.{}", quote_ident(primary(current)));
    loop {
        if visited.contains(&current.entity_name.as_str()) || visited.len() >= MAX_OWNER_CHAIN {
            return Err(invalid(format!("share: ownership chain from '{start}' loops or exceeds MAX_OWNER_CHAIN")));
        }
        visited.push(&current.entity_name);
        let index = visited.len() - 1;
        let owner = owner_field(current).ok_or_else(|| invalid(format!("share: entity '{}' has no owner", current.entity_name)))?;
        let identity = format!("{prefix}{index}.{}", quote_ident(owner));
        let Some(parent) = owner_parent(current) else {
            let field = current.fields.iter().find(|f| f.name == owner).unwrap();
            let identity_type = if field.field_type == "text" { "text" } else { "uuid" };
            return Ok(Path { from, identity, identity_type, pk });
        };
        let target = link(manifest, current, owner)?;
        let next = index + 1;
        from.push_str(&format!(
            " JOIN {}.{} {prefix}{next} ON {identity} = {prefix}{next}.{}",
            quote_ident(&manifest.app_id), quote_ident(parent), quote_ident(primary(target)),
        ));
        current = target;
    }
}

pub(super) async fn declare(
    conn: &mut PgConnection, manifest: &AppManifest, table: &str, owners: &OwnerMap,
) -> Result<(String, String), RuntimeError> {
    let schema = &manifest.app_id;
    let mut subjects = Vec::new();
    for source in &manifest.data_contract {
        let Some(share) = &source.share else { continue };
        let grantee = path(manifest, &link(manifest, source, &share.grantee)?.entity_name, "g")?;
        let subject = path(manifest, &link(manifest, source, &share.subject)?.entity_name, "s")?;
        // Legacy text ownership may contain non-user values. Such values are not
        // Core identities, and must not cause a cast failure on another table.
        let identity = if subject.identity_type == "text" {
            format!("CASE WHEN {} ~ '^[0-9a-f]{{8}}-[0-9a-f]{{4}}-[0-9a-f]{{4}}-[0-9a-f]{{4}}-[0-9a-f]{{12}}$' THEN {} END", subject.identity, subject.identity)
        } else { format!("{}::text", subject.identity) };
        subjects.push(format!(
            "SELECT {identity} AS identity FROM {}.{} a, {}, {}
             WHERE a.{} = {} AND a.{} = {} AND a.{} IS NULL AND {} =
             (SELECT nullif(current_setting('rootcx.user_id', true), ''))::{}",
            quote_ident(schema), quote_ident(&source.entity_name),
            grantee.from, subject.from, quote_ident(&share.grantee), grantee.pk,
            quote_ident(&share.subject), subject.pk,
            quote_ident(&share.active_when.is_null), grantee.identity, grantee.identity_type,
        ));
    }
    let (column, parent) = &owners[table];
    let ty = policies::column_type(conn, schema, table, column).await?
        .ok_or_else(|| invalid(format!("share: column '{schema}.{table}.{column}' is missing")))?;
    let read_key = quote_literal(&format!("app:{schema}:{table}.read.shared"));
    let guard = format!(
        "(SELECT rootcx_system.check_access({read_key})) AND
         (current_setting('rootcx.human_data_request', true) = '1' OR
          nullif(current_setting('rootcx.app_id', true), '') = {})",
        quote_literal(schema),
    );
    let select = if let Some(parent) = parent {
        let target = path(manifest, parent, "t")?;
        format!(
            "SELECT DISTINCT {} FROM {} WHERE {guard} AND {} IN
             (SELECT identity::{} FROM subjects WHERE identity IS NOT NULL)",
            target.pk, target.from, target.identity, target.identity_type,
        )
    } else {
        format!("SELECT DISTINCT identity::{ty} FROM subjects WHERE {guard} AND identity IS NOT NULL")
    };
    let resolver = format!("rootcx_shared.{schema}.{table}");
    let signature = format!("rootcx_system.{}()", quote_ident(&resolver));
    // Drop first: a changed ownership column can change the return type.
    // Policies refer to this function, so rebuild its known dependent first.
    exec(conn, &format!("DROP POLICY IF EXISTS rootcx_rls_select_shared ON {}.{}", quote_ident(schema), quote_ident(table))).await?;
    exec(conn, &format!("DROP FUNCTION IF EXISTS {signature}")).await?;
    exec(conn, &format!(
        "CREATE FUNCTION {signature} RETURNS SETOF {ty} LANGUAGE sql STABLE
         SECURITY DEFINER SET search_path = pg_catalog AS $rootcx$
         WITH subjects AS ({}) {select} $rootcx$", subjects.join(" UNION ALL "),
    )).await?;
    exec(conn, &format!("REVOKE ALL ON FUNCTION {signature} FROM PUBLIC")).await?;
    exec(conn, &format!("GRANT EXECUTE ON FUNCTION {signature} TO rootcx_app_executor")).await?;

    Ok((format!("{} IN (SELECT {signature})", quote_ident(column)), resolver))
}

pub(super) async fn reconcile_indexes(conn: &mut PgConnection, manifest: &AppManifest) -> Result<(), RuntimeError> {
    let prefix = format!("rootcx_share_{}_", manifest.app_id);
    let indexes: Vec<String> = sqlx::query_scalar(
        "SELECT indexname FROM pg_indexes WHERE schemaname = $1",
    ).bind(&manifest.app_id).fetch_all(&mut *conn).await.map_err(RuntimeError::Schema)?;
    for name in &indexes {
        let Some(table) = name.strip_prefix(&prefix) else { continue };
        let declaration = manifest.data_contract.iter().find(|e| e.entity_name == table)
            .and_then(|e| e.share.as_ref());
        // Keep unchanged index definitions using a Core-owned description of the
        // declaration. Admission inspects the actual expression, not this comment.
        let expected = declaration.map(|s| serde_json::to_string(s).unwrap());
        let stored: Option<String> = sqlx::query_scalar(
            "SELECT obj_description(to_regclass($1), 'pg_class')",
        ).bind(format!("{}.{}", quote_ident(&manifest.app_id), quote_ident(name)))
            .fetch_one(&mut *conn).await.map_err(RuntimeError::Schema)?;
        if expected.is_none() || expected != stored {
            exec(conn, &format!("DROP INDEX {}.{}", quote_ident(&manifest.app_id), quote_ident(name))).await?;
        }
    }
    for source in &manifest.data_contract {
        let Some(share) = &source.share else { continue };
        let name = format!("{prefix}{}", source.entity_name);
        exec(conn, &format!(
            "CREATE INDEX IF NOT EXISTS {} ON {}.{} ({}, {}) WHERE {} IS NULL",
            quote_ident(&name), quote_ident(&manifest.app_id), quote_ident(&source.entity_name),
            quote_ident(&share.grantee), quote_ident(&share.subject), quote_ident(&share.active_when.is_null),
        )).await?;
        exec(conn, &format!("COMMENT ON INDEX {}.{} IS {}",
            quote_ident(&manifest.app_id), quote_ident(&name),
            quote_literal(&serde_json::to_string(share).map_err(|e| invalid(e.to_string()))?),
        )).await?;
    }
    Ok(())
}

pub(super) async fn prune(conn: &mut PgConnection, schema: &str, keep: &[String]) -> Result<(), RuntimeError> {
    let names: Vec<String> = sqlx::query_scalar(
        "SELECT p.proname FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace
         WHERE n.nspname = 'rootcx_system' AND
         p.proname LIKE 'rootcx\\_shared.%'",
    ).fetch_all(&mut *conn).await.map_err(RuntimeError::Schema)?;
    let prefix = format!("rootcx_shared.{schema}.");
    for name in names {
        if name.starts_with(&prefix) && !keep.contains(&name) {
            exec(conn, &format!("DROP FUNCTION rootcx_system.{}()", quote_ident(&name))).await?;
        }
    }
    Ok(())
}
