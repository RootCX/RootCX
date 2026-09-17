use std::collections::HashSet;

use rootcx_types::{AppManifest, EntityContract, ShareScope, ShareTarget};
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
    Ok(target)
}

fn owned_link<'a>(manifest: &'a AppManifest, source: &EntityContract, field: &str) -> Result<&'a EntityContract, RuntimeError> {
    let target = link(manifest, source, field)?;
    if owner_field(target).is_none() {
        return Err(invalid(format!("entity '{}': share field '{}' targets '{}', which declares no owner field", source.entity_name, field, target.entity_name)));
    }
    Ok(target)
}

/// Permission discovery and reconciliation use the same target selection.
/// Resource sharing never propagates implicitly through ownership.
pub(super) fn is_target(manifest: &AppManifest, target: &EntityContract) -> bool {
    manifest.data_contract.iter().any(|source| {
        let Some(share) = &source.share else { return false };
        match share.scope {
            ShareScope::Identity => owner_field(target).is_some(),
            ShareScope::Resource => {
                source.fields.iter().find(|f| f.name == share.subject)
                    .and_then(|f| f.references.as_ref())
                    .is_some_and(|r| r.entity == target.entity_name)
                    || share.targets.iter().any(|t| t.entity == target.entity_name)
            }
        }
    })
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
        let grantee = owned_link(manifest, source, &share.grantee)?;
        path(manifest, &grantee.entity_name, "g")?;
        match share.scope {
            ShareScope::Identity => {
                owned_link(manifest, source, &share.subject)?;
                if !share.targets.is_empty() {
                    return Err(invalid("share targets require scope 'resource'"));
                }
            }
            ShareScope::Resource => {
                let subject = link(manifest, source, &share.subject)?;
                readable_key(subject, primary(subject))?;
                if share.targets.len() > 32 {
                    return Err(invalid("resource share supports at most 32 target paths"));
                }
                let mut seen = HashSet::new();
                for target in &share.targets {
                    if !seen.insert((&target.entity, &target.via)) {
                        return Err(invalid(format!("duplicate resource share path for '{}'", target.entity)));
                    }
                    resource_path(manifest, subject, target)?;
                }
            }
        }
        let active = source.fields.iter().find(|f| f.name == share.active_when.is_null)
            .ok_or_else(|| invalid(format!("entity '{}': share activeWhen.isNull names missing field '{}'", source.entity_name, share.active_when.is_null)))?;
        if !matches!(active.field_type.as_str(), "date" | "timestamp") || active.required {
            return Err(invalid(format!("entity '{}': share activeWhen.isNull field '{}' must be a nullable date or timestamp", source.entity_name, active.name)));
        }
    }
    // Preserve the existing sensitive-owner restriction for all sharing apps,
    // including grantees that are not themselves resource targets.
    if manifest.data_contract.iter().any(|e| e.share.is_some()) {
        for owned in &manifest.data_contract {
            // Callable ownership resolvers can return these keys directly.
            if owned.fields.iter().any(|field| field.owner && field.sensitive) {
                return Err(invalid(format!(
                    "entity '{}': shared access requires a nonsensitive owner field; its resolver returns ownership keys",
                    owned.entity_name,
                )));
            }
        }
    }
    for target in shared_entities(manifest) {
        let resolver = format!("rootcx_shared.{}.{}", manifest.app_id, target.entity_name);
        if !fits_ident_limit(&resolver) {
            return Err(invalid(format!("share resolver '{resolver}' exceeds PostgreSQL's 63-byte identifier limit")));
        }
        // Follow the same ownership graph; sharing never recursively follows
        // another share and never imports another grantee's authority.
        if owner_field(target).is_some() {
            path(manifest, &target.entity_name, "v")?;
        }
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
        let target = owned_link(manifest, current, owner)?;
        let next = index + 1;
        from.push_str(&format!(
            " JOIN {}.{} {prefix}{next} ON {identity} = {prefix}{next}.{}",
            quote_ident(&manifest.app_id), quote_ident(parent), quote_ident(primary(target)),
        ));
        current = target;
    }
}

const MAX_RESOURCE_ENTITIES: usize = 4;

fn readable_key(entity: &EntityContract, name: &str) -> Result<(), RuntimeError> {
    if entity.fields.iter().any(|f| f.name == name && f.sensitive) {
        return Err(invalid(format!(
            "resource share requires a nonsensitive key '{}.{}'", entity.entity_name, name
        )));
    }
    Ok(())
}

struct ResourcePath {
    from: String,
    pk: String,
    resource: String,
}

/// Only explicit local primary-key links are traversed. No ownership or other
/// share declaration is consulted, and no materialized access rows are needed.
fn resource_path(
    manifest: &AppManifest, subject: &EntityContract, target: &ShareTarget,
) -> Result<ResourcePath, RuntimeError> {
    if target.via.is_empty() || target.via.len() >= MAX_RESOURCE_ENTITIES {
        return Err(invalid("resource share path must contain 1 to 3 local links"));
    }
    let mut current = entity(manifest, &target.entity)?;
    readable_key(current, primary(current))?;
    let pk = format!("t0.{}", quote_ident(primary(current)));
    let mut from = format!("{}.{} t0", quote_ident(&manifest.app_id), quote_ident(&current.entity_name));
    let mut visited = HashSet::from([current.entity_name.as_str()]);
    for (index, field) in target.via.iter().enumerate() {
        readable_key(current, field)?;
        let next = link(manifest, current, field)?;
        readable_key(next, primary(next))?;
        if !visited.insert(next.entity_name.as_str()) {
            return Err(invalid("resource share path must not loop or repeat an entity"));
        }
        from.push_str(&format!(
            " JOIN {}.{} t{} ON t{}.{} = t{}.{}",
            quote_ident(&manifest.app_id), quote_ident(&next.entity_name), index + 1,
            index, quote_ident(field), index + 1, quote_ident(primary(next)),
        ));
        current = next;
    }
    if current.entity_name != subject.entity_name {
        return Err(invalid(format!(
            "resource share path from '{}' must end at subject entity '{}'",
            target.entity, subject.entity_name
        )));
    }
    Ok(ResourcePath {
        from, pk,
        resource: format!("t{}.{}", target.via.len(), quote_ident(primary(subject))),
    })
}

fn guard(schema: &str, table: &str) -> String {
    let read_key = quote_literal(&format!("app:{schema}:{table}.read.shared"));
    format!(
        "(SELECT rootcx_system.check_access({read_key})) AND
         (current_setting('rootcx.human_data_request', true) = '1' OR
          nullif(current_setting('rootcx.app_id', true), '') = {})",
        quote_literal(schema),
    )
}

fn resource_selects(manifest: &AppManifest, table: &str) -> Result<Vec<String>, RuntimeError> {
    let mut selects = Vec::new();
    for source in &manifest.data_contract {
        let Some(share) = source.share.as_ref().filter(|s| s.scope == ShareScope::Resource) else { continue };
        let subject = link(manifest, source, &share.subject)?;
        let grantee = path(manifest, &owned_link(manifest, source, &share.grantee)?.entity_name, "g")?;
        let mut paths = Vec::new();
        if subject.entity_name == table {
            let pk = format!("t0.{}", quote_ident(primary(subject)));
            paths.push(ResourcePath {
                from: format!("{}.{} t0", quote_ident(&manifest.app_id), quote_ident(table)),
                pk: pk.clone(), resource: pk,
            });
        }
        for target in share.targets.iter().filter(|t| t.entity == table) {
            paths.push(resource_path(manifest, subject, target)?);
        }
        for target in paths {
            selects.push(format!(
                "SELECT {} FROM {}.{} a, {}, {}
                 WHERE a.{} = {} AND a.{} = {} AND a.{} IS NULL AND {} =
                 (SELECT nullif(current_setting('rootcx.user_id', true), ''))::{}
                 AND ({})",
                target.pk, quote_ident(&manifest.app_id), quote_ident(&source.entity_name),
                grantee.from, target.from, quote_ident(&share.grantee), grantee.pk,
                quote_ident(&share.subject), target.resource,
                quote_ident(&share.active_when.is_null), grantee.identity, grantee.identity_type,
                guard(&manifest.app_id, table),
            ));
        }
    }
    Ok(selects)
}

pub(super) async fn declare(
    conn: &mut PgConnection, manifest: &AppManifest, table: &str, owners: &OwnerMap,
) -> Result<(String, String), RuntimeError> {
    let schema = &manifest.app_id;
    let mut selects = resource_selects(manifest, table)?;
    let legacy = identity_select(manifest, table, owners, conn).await?;
    let SharedSelect { column, ty, select } = if selects.is_empty() {
        legacy.ok_or_else(|| invalid(format!("share: entity '{table}' has no shared read declaration")))?
    } else {
        let target = entity(manifest, table)?;
        let column = primary(target).to_string();
        let ty = policies::column_type(conn, schema, table, &column).await?
            .ok_or_else(|| invalid(format!("share: primary key '{schema}.{table}.{column}' is missing")))?;
        if let Some(SharedSelect { column: owner, select, .. }) = legacy {
            // Mixed declarations add independent authority. Convert legacy
            // owner keys to row keys without changing legacy-only resolvers.
            selects.push(format!(
                "SELECT {} FROM {}.{} WHERE {} IN ({select})",
                quote_ident(&column), quote_ident(schema), quote_ident(table), quote_ident(&owner),
            ));
        }
        SharedSelect { column, ty, select: selects.join(" UNION ") }
    };
    let resolver = format!("rootcx_shared.{schema}.{table}");
    let signature = format!("rootcx_system.{}()", quote_ident(&resolver));
    // Drop first: a changed declaration can change the resolver's return type.
    exec(conn, &format!("DROP POLICY IF EXISTS rootcx_rls_select_shared ON {}.{}", quote_ident(schema), quote_ident(table))).await?;
    exec(conn, &format!("DROP FUNCTION IF EXISTS {signature}")).await?;
    exec(conn, &format!(
        "CREATE FUNCTION {signature} RETURNS SETOF {ty} LANGUAGE sql STABLE
         SECURITY DEFINER SET search_path = pg_catalog AS $rootcx$
         {select} $rootcx$",
    )).await?;
    exec(conn, &format!("REVOKE ALL ON FUNCTION {signature} FROM PUBLIC")).await?;
    exec(conn, &format!("GRANT EXECUTE ON FUNCTION {signature} TO rootcx_app_executor")).await?;

    Ok((format!("{} IN (SELECT {signature})", quote_ident(&column)), resolver))
}

struct SharedSelect {
    column: String,
    ty: String,
    select: String,
}

async fn identity_select(
    manifest: &AppManifest, table: &str, owners: &OwnerMap, conn: &mut PgConnection,
) -> Result<Option<SharedSelect>, RuntimeError> {
    let Some((column, parent)) = owners.get(table) else { return Ok(None) };
    let schema = &manifest.app_id;
    let mut subjects = Vec::new();
    for source in &manifest.data_contract {
        let Some(share) = source.share.as_ref().filter(|s| s.scope == ShareScope::Identity) else { continue };
        let grantee = path(manifest, &owned_link(manifest, source, &share.grantee)?.entity_name, "g")?;
        let subject = path(manifest, &owned_link(manifest, source, &share.subject)?.entity_name, "s")?;
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
    if subjects.is_empty() { return Ok(None) }
    let ty = policies::column_type(conn, schema, table, column).await?
        .ok_or_else(|| invalid(format!("share: column '{schema}.{table}.{column}' is missing")))?;
    let guard = guard(schema, table);
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
    Ok(Some(SharedSelect {
        column: column.clone(),
        ty,
        select: format!("WITH subjects AS ({}) {select}", subjects.join(" UNION ALL ")),
    }))
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn resource_manifest() -> Value {
        json!({
            "appId": "workspace", "name": "Workspace",
            "dataContract": [
                {"entityName": "member", "fields": [
                    {"name": "user_id", "type": "uuid", "owner": true}
                ]},
                {"entityName": "project", "fields": [
                    {"name": "label", "type": "text"},
                    {"name": "document_id", "type": "entity_link",
                     "references": {"entity": "document", "field": "id"}}
                ]},
                {"entityName": "folder", "fields": [
                    {"name": "project_id", "type": "entity_link",
                     "references": {"entity": "project", "field": "id"}}
                ]},
                {"entityName": "document", "fields": [
                    {"name": "folder_id", "type": "entity_link",
                     "references": {"entity": "folder", "field": "id"}}
                ]},
                {"entityName": "access", "fields": [
                    {"name": "member_id", "type": "entity_link",
                     "references": {"entity": "member", "field": "id"}},
                    {"name": "project_id", "type": "entity_link",
                     "references": {"entity": "project", "field": "id"}},
                    {"name": "ended_at", "type": "timestamp"}
                ], "share": {
                    "scope": "resource", "grantee": "member_id", "subject": "project_id",
                    "activeWhen": {"isNull": "ended_at"},
                    "targets": [{"entity": "document", "via": ["folder_id", "project_id"]}]
                }}
            ]
        })
    }

    #[test]
    fn resource_sharing_targets_only_explicit_entities_without_requiring_subject_ownership() {
        let manifest: AppManifest = serde_json::from_value(resource_manifest()).unwrap();
        crate::manifest::validate_manifest(&manifest).unwrap();
        let targets: Vec<_> = shared_entities(&manifest).map(|e| e.entity_name.as_str()).collect();
        assert_eq!(targets, ["project", "document"], "intermediates and grantees must not inherit sharing");
    }

    #[test]
    fn resource_sharing_rejects_unbounded_or_ambiguous_paths() {
        let cases = [
            ("/dataContract/4/share/scope", json!("identity"), "no owner field"),
            ("/dataContract/4/share/targets/0/via", json!([]), "1 to 3 local links"),
            ("/dataContract/4/share/targets/0/via", json!(["folder_id", "project_id", "document_id", "folder_id"]), "1 to 3 local links"),
            ("/dataContract/4/share/targets/0/via", json!(["folder_id", "project_id", "document_id"]), "repeat an entity"),
            ("/dataContract/4/share/targets/0/via", json!(["folder_id"]), "must end at subject"),
            ("/dataContract/4/share/targets/0/via", json!(["absent"]), "does not exist"),
            ("/dataContract/4/share/targets/0/entity", json!("absent"), "unknown entity"),
            ("/dataContract/4/share/targets", json!([
                {"entity": "document", "via": ["folder_id", "project_id"]},
                {"entity": "document", "via": ["folder_id", "project_id"]}
            ]), "duplicate"),
            ("/dataContract/2/fields/0", json!({
                "name": "project_id", "type": "entity_link", "sensitive": true,
                "references": {"entity": "project", "field": "id"}
            }), "nonsensitive key"),
            ("/dataContract/2/fields/0/type", json!("text"), "local entity_link"),
            ("/dataContract/2/fields/0/references/entity", json!("core:users"), "local entity"),
            ("/dataContract/2/fields/0/references/field", json!("label"), "primary key"),
        ];
        for (pointer, value, expected) in cases {
            let mut input = resource_manifest();
            *input.pointer_mut(pointer).unwrap() = value;
            let manifest: AppManifest = serde_json::from_value(input).unwrap();
            let error = validate(&manifest).expect_err(pointer).to_string();
            assert!(error.contains(expected), "{pointer}: expected {expected:?}, got {error}");
        }
        let mut input = resource_manifest();
        input["dataContract"][4]["share"]["targets"] = json!(
            (0..33).map(|n| json!({"entity": format!("target_{n}"), "via": ["project_id"]})).collect::<Vec<_>>()
        );
        let manifest: AppManifest = serde_json::from_value(input).unwrap();
        assert!(validate(&manifest).unwrap_err().to_string().contains("at most 32"));
    }

    #[test]
    fn identity_sharing_cannot_silently_ignore_resource_targets() {
        let mut input = resource_manifest();
        input["dataContract"][4]["share"]["scope"] = json!("identity");
        input["dataContract"][1]["fields"].as_array_mut().unwrap()
            .push(json!({"name": "owner_id", "type": "uuid", "owner": true}));
        let manifest: AppManifest = serde_json::from_value(input).unwrap();
        assert!(validate(&manifest).unwrap_err().to_string().contains("targets require scope 'resource'"));
    }
}
