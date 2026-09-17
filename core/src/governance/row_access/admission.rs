//! Validate new app SQL declarations before privileged schema DDL.
//! Existing database objects are operator-managed; boot does not audit them.

use rootcx_types::{AppManifest, IndexColumn};
use crate::RuntimeError;

pub(crate) fn validate_manifest(manifest: &AppManifest) -> Result<(), RuntimeError> {
    for entity in &manifest.data_contract {
        let reject = |reason: &str| RuntimeError::Invalid(format!(
            "SQL admission for '{}.{}': {reason}", manifest.app_id, entity.entity_name,
        ));
        if !entity.checks.is_empty() {
            return Err(reject(
                "raw CHECK expressions are not supported; use field enums",
            ));
        }
        for index in &entity.indexes {
            if index.where_clause.is_some() || !index.with.is_empty() {
                return Err(reject(
                    "raw index predicates and storage parameters are not supported",
                ));
            }
            for column in &index.columns {
                let name = match column {
                    IndexColumn::Name(name) => name,
                    IndexColumn::Spec(spec) => {
                        if spec.expr.is_some() || spec.ops.is_some() {
                            return Err(reject(
                                "index expressions and operator classes are not supported",
                            ));
                        }
                        spec.column
                            .as_ref()
                            .ok_or_else(|| reject("index column is missing"))?
                    }
                };
                if !crate::manifest::is_system_field(name)
                    && !entity.fields.iter().any(|field| field.name == *name)
                {
                    return Err(reject("index column is not a declared or system field"));
                }
            }
            // Reuse the renderer's closed vocabularies and shape checks, before
            // DDL rather than after creating tables or dropping old indexes.
            crate::schema_sync::generate_create_index(&manifest.app_id, &entity.entity_name, index)
                .map_err(|error| reject(&error))?;
        }
    }
    Ok(())
}
