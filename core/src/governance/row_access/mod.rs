//! The Core-owned data contract: validate once, reconcile atomically, replay at
//! boot. Apps supply relationships, never SQL authorization expressions.
use std::collections::HashMap;

use rootcx_types::{AppManifest, EntityContract, ShareScope};
use sqlx::{PgConnection, PgPool};

use crate::RuntimeError;

mod admission;
mod ownership;
mod policies;
mod sharing;

pub(crate) use admission::validate_manifest as validate_sql_declarations;
pub(crate) use ownership::{owner_field, owner_parent, owner_map, MAX_OWNER_CHAIN, validate_owner_chains, validate_owner_field};
pub(crate) use policies::{OwnerMap, fits_ident_limit, owner_resolver_name};

async fn exec(conn: &mut PgConnection, sql: &str) -> Result<(), RuntimeError> {
    sqlx::query(sql).execute(conn).await.map_err(RuntimeError::Schema)?;
    Ok(())
}

pub(crate) fn validate(manifest: &AppManifest) -> Result<(), RuntimeError> {
    for entity in &manifest.data_contract {
        validate_owner_field(entity).map_err(RuntimeError::Invalid)?;
    }
    validate_owner_chains(manifest).map_err(RuntimeError::Invalid)?;
    sharing::validate(manifest)
}

pub(crate) fn shared_entities(manifest: &AppManifest) -> impl Iterator<Item = &EntityContract> {
    manifest.data_contract.iter().filter(move |e| sharing::is_target(manifest, e))
}

fn contract_version(manifest: &AppManifest) -> i32 {
    if manifest.data_contract.iter().any(|e| e.share.as_ref().is_some_and(|s| s.scope == ShareScope::Resource)) {
        2
    } else {
        1
    }
}

/// Bootstrap only the projection. Triggers need it before RBAC itself boots.
pub(crate) async fn bootstrap_projection(pool: &PgPool) -> Result<(), RuntimeError> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS rootcx_system.row_access_contracts (
            app_id TEXT PRIMARY KEY, version INTEGER NOT NULL,
            entities JSONB NOT NULL
        )",
    ).execute(pool).await.map_err(RuntimeError::Schema)?;
    // Version 1 remains readable; resource declarations require version 2.
    // Upgrade the old Core-owned check without touching app data.
    let mut tx = pool.begin().await.map_err(RuntimeError::Schema)?;
    exec(&mut tx, "ALTER TABLE rootcx_system.row_access_contracts
        DROP CONSTRAINT IF EXISTS row_access_contracts_version_check").await?;
    exec(&mut tx, "ALTER TABLE rootcx_system.row_access_contracts
        ADD CONSTRAINT row_access_contracts_version_check CHECK (version IN (1, 2))").await?;
    tx.commit().await.map_err(RuntimeError::Schema)
}

pub(crate) async fn install(pool: &PgPool, manifest: &AppManifest) -> Result<(), RuntimeError> {
    validate(manifest)?;
    let mut tx = pool.begin().await.map_err(RuntimeError::Schema)?;
    reconcile(&mut tx, manifest).await?;
    sqlx::query(
        "INSERT INTO rootcx_system.row_access_contracts (app_id, version, entities)
         VALUES ($1, $2, $3) ON CONFLICT (app_id) DO UPDATE
         SET version = EXCLUDED.version, entities = EXCLUDED.entities",
    )
    .bind(&manifest.app_id)
    .bind(contract_version(manifest))
    .bind(serde_json::to_value(&manifest.data_contract).map_err(|e| RuntimeError::Invalid(e.to_string()))?)
    .execute(&mut *tx).await.map_err(RuntimeError::Schema)?;
    tx.commit().await.map_err(RuntimeError::Schema)
}

async fn reconcile(conn: &mut PgConnection, manifest: &AppManifest) -> Result<(), RuntimeError> {
    let schema = &manifest.app_id;
    let owners = owner_map(&manifest.data_contract);
    let entities: HashMap<_, _> = manifest.data_contract.iter()
        .map(|e| (e.entity_name.as_str(), e)).collect();
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT tablename FROM pg_tables WHERE schemaname = $1 ORDER BY tablename",
    ).bind(schema).fetch_all(&mut *conn).await.map_err(RuntimeError::Schema)?;
    for entity in &manifest.data_contract {
        if !tables.contains(&entity.entity_name) {
            return Err(RuntimeError::Invalid(format!(
                "governance: declared table '{schema}.{}' is missing; redeploy its manifest",
                entity.entity_name,
            )));
        }
    }
    let mut shared_resolvers = Vec::new();
    sharing::reconcile_indexes(conn, manifest).await?;
    for table in &tables {
        let sensitive: Vec<String> = entities.get(table.as_str()).into_iter()
            .flat_map(|e| &e.fields).filter(|f| f.sensitive).map(|f| f.name.clone()).collect();
        let shared = if entities.get(table.as_str()).is_some_and(|e| sharing::is_target(manifest, e)) {
            let (predicate, resolver) = sharing::declare(conn, manifest, table, &owners).await?;
            shared_resolvers.push(resolver);
            Some(predicate)
        } else {
            None
        };
        let approved_read = manifest.actions.iter()
            .filter_map(|a| a.authority.as_ref().and_then(|authority| authority.data.get(table)))
            .any(|operations| operations.contains(&rootcx_types::DataOperation::Read));
        policies::apply_table_rls(conn, schema, table, &owners, shared.as_deref(), &sensitive, approved_read).await?;
    }
    let parents: Vec<String> = owners.values().filter_map(|(_, p)| p.clone()).collect();
    policies::prune_owner_resolvers_tx(conn, schema, &parents).await?;
    sharing::prune(conn, schema, &shared_resolvers).await?;

    // Audit and hooks consume this projection, but governance is its sole writer.
    sqlx::query("DELETE FROM rootcx_system.sensitive_fields WHERE app_id = $1")
        .bind(schema).execute(&mut *conn).await.map_err(RuntimeError::Schema)?;
    for entity in &manifest.data_contract {
        let fields: Vec<&str> = entity.fields.iter().filter(|f| f.sensitive)
            .map(|f| f.name.as_str()).collect();
        let owner = owner_field(entity);
        if fields.is_empty() && owner.is_none() { continue; }
        sqlx::query(
            "INSERT INTO rootcx_system.sensitive_fields (app_id, entity, fields, owner_field, owner_parent)
             VALUES ($1, $2, $3, $4, $5)",
        ).bind(schema).bind(&entity.entity_name).bind(fields).bind(owner).bind(owner_parent(entity))
            .execute(&mut *conn).await.map_err(RuntimeError::Schema)?;
    }
    Ok(())
}

/// Replay access rules for apps with tables, declared entities or a saved contract.
/// Registrations without data (such as workflows) have no row policies to replay.
pub(crate) async fn govern_schema_tables(pool: &PgPool, only: Option<&str>) -> Result<(), RuntimeError> {
    bootstrap_projection(pool).await?;
    let apps: Vec<(String, Option<serde_json::Value>, Option<i32>, Option<serde_json::Value>)> = sqlx::query_as(
        "SELECT a.id, a.manifest, c.version, c.entities FROM rootcx_system.apps a
         LEFT JOIN rootcx_system.row_access_contracts c ON c.app_id = a.id
         WHERE a.id <> 'core' AND ($1::text IS NULL OR a.id = $1)
           AND (c.app_id IS NOT NULL
             OR EXISTS (SELECT 1 FROM pg_tables WHERE schemaname = a.id)
             OR COALESCE(a.manifest->'dataContract', '[]'::jsonb) <> '[]'::jsonb)
         ORDER BY a.id",
    ).bind(only).fetch_all(pool).await.map_err(RuntimeError::Schema)?;
    for (app, json, version, entities) in apps {
        let json = json.ok_or_else(|| RuntimeError::Invalid(format!(
            "governance for '{app}': restore the missing manifest before governing its data"
        )))?;
        let mut manifest: AppManifest = serde_json::from_value(json)
            .map_err(|e| RuntimeError::Invalid(format!("governance for '{app}': invalid stored manifest: {e}")))?;
        if let Some(version) = version {
            if !matches!(version, 1 | 2) {
                return Err(RuntimeError::Invalid(format!("governance for '{app}': unsupported contract version {version}")));
            }
            manifest.data_contract = serde_json::from_value(entities.unwrap_or_default())
                .map_err(|e| RuntimeError::Invalid(format!("governance for '{app}': invalid projection: {e}")))?;
            if version < contract_version(&manifest) {
                return Err(RuntimeError::Invalid(format!(
                    "governance for '{app}': resource sharing requires contract version 2"
                )));
            }
        }
        crate::manifest::validate_stored_manifest(&manifest)?;
        install(pool, &manifest).await?;
    }
    Ok(())
}

pub(crate) async fn prune_owner_resolvers(pool: &PgPool, schema: &str, keep: &[String]) -> Result<(), RuntimeError> {
    let mut tx = pool.begin().await.map_err(RuntimeError::Schema)?;
    policies::prune_owner_resolvers_tx(&mut tx, schema, keep).await?;
    if keep.is_empty() {
        sharing::prune(&mut tx, schema, &[]).await?;
        sqlx::query("DELETE FROM rootcx_system.row_access_contracts WHERE app_id = $1")
            .bind(schema).execute(&mut *tx).await.map_err(RuntimeError::Schema)?;
        sqlx::query("DELETE FROM rootcx_system.rbac_permissions WHERE source_app = $1")
            .bind(schema).execute(&mut *tx).await.map_err(RuntimeError::Schema)?;
    }
    tx.commit().await.map_err(RuntimeError::Schema)
}
