//! Admit app rule declarations before privileged schema DDL (ADR 0010).
//!
//! Checks, index predicates and index expressions are programs in the governed
//! rule language, which `crate::rules` parses, type-checks and compiles; no app
//! string reaches SQL. Operator classes and storage parameters stay refused.
//! Existing database objects are operator-managed; boot does not audit them.

use rootcx_types::AppManifest;
use crate::RuntimeError;

pub(crate) fn validate_manifest(manifest: &AppManifest) -> Result<(), RuntimeError> {
    for entity in &manifest.data_contract {
        crate::rules::compile_entity(entity).map_err(|reason| RuntimeError::Invalid(format!(
            "SQL admission for '{}.{}': {reason}", manifest.app_id, entity.entity_name,
        )))?;
    }
    Ok(())
}
