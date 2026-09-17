use std::collections::HashMap;
use rootcx_types::{AppManifest, EntityContract, FieldContract};
use crate::manifest::{parse_entity_ref, RefTarget};

/// The column deciding who a row belongs to, when the entity declares one.
///
/// Either it holds the user id itself — an `entity_link` to `core:users` (typed
/// UUID, and it earns a foreign key and index for free), a bare `uuid`, or `text`
/// — or it links to another entity that carries the ownership (see
/// [`owner_parent`]). Text is not a concession: the bundled gmail, google_calendar
/// and imap_smtp integrations store `user_id` as text across nine tables, and
/// confining those per user is the same need. The generated policy casts the
/// *caller's* id to the column's type rather than the column to text, so each shape
/// stays indexable (see `rbac::owner_predicate`).
pub(crate) fn owner_field(entity: &EntityContract) -> Option<&str> {
    entity
        .fields
        .iter()
        .find(|f| f.owner)
        .map(|f| f.name.as_str())
}

/// The entity this one delegates ownership to, when its owner column is a link to
/// a sibling entity rather than a user id. `None` covers both "no owner declared"
/// and "owns directly", which is exactly the distinction the policy builder needs.
pub(crate) fn owner_parent(entity: &EntityContract) -> Option<&str> {
    let field = entity.fields.iter().find(|f| f.owner)?;
    let target = &field.references.as_ref()?.entity;
    (field.field_type == "entity_link" && matches!(parse_entity_ref(target), RefTarget::Local(_)))
        .then_some(target.as_str())
}

/// Every entity's ownership as the policy builder consumes it: the column, and the
/// sibling it defers to. Built once from the manifest at install and once from the
/// projection at boot, so both paths generate the same SQL from the same shape.
pub(crate) fn owner_map(entities: &[EntityContract]) -> super::OwnerMap {
    entities
        .iter()
        .filter_map(|e| {
            let column = owner_field(e)?.to_string();
            Some((e.entity_name.clone(), (column, owner_parent(e).map(str::to_string))))
        })
        .collect()
}

/// How many entities a delegation chain may span. Each extra link is one more
/// resolver the planner must run per confined query, and a chain this long is
/// already a modelling smell — so it is a refusal at install rather than a
/// surprise in production.
pub(crate) const MAX_OWNER_CHAIN: usize = 4;

/// Delegated ownership must terminate in a real user id, and must do so in bounded
/// time. Cross-entity, so it cannot live in `validate_owner_field`.
///
/// Every failure here would otherwise surface as a broken *table*: a chain that
/// loops makes Postgres report `infinite recursion detected in policy` only when
/// the table is first queried, and until the manifest is fixed the table cannot be
/// read at all. A chain ending on an entity that owns nothing is quieter and worse
/// — the policies simply match nothing, which reads as an access bug.
pub(crate) fn validate_owner_chains(manifest: &AppManifest) -> Result<(), String> {
    let by_name: HashMap<&str, &EntityContract> =
        manifest.data_contract.iter().map(|e| (e.entity_name.as_str(), e)).collect();

    for entity in &manifest.data_contract {
        let Some(mut parent) = owner_parent(entity) else { continue };
        let target = by_name.get(parent).ok_or_else(|| format!(
            "entity '{}' delegates ownership to unknown entity '{parent}'", entity.entity_name
        ))?;
        let target_pk = target.fields.iter()
            .find(|f| f.is_primary_key == Some(true) || f.name == "id")
            .map(|f| f.name.as_str()).unwrap_or("id");
        let reference = entity.fields.iter().find(|f| f.owner)
            .and_then(|f| f.references.as_ref()).unwrap();
        if reference.field != target_pk {
            return Err(format!("entity '{}': ownership must reference primary key '{parent}.{target_pk}'",
                entity.entity_name));
        }
        let mut chain = vec![entity.entity_name.as_str()];
        loop {
            if chain.contains(&parent) {
                chain.push(parent);
                return Err(format!(
                    "entity '{}' delegates ownership in a loop ({}); a chain must end on a \
                     column holding a user id",
                    entity.entity_name, chain.join(" -> "),
                ));
            }
            chain.push(parent);
            if chain.len() > MAX_OWNER_CHAIN {
                return Err(format!(
                    "entity '{}' delegates ownership through {} entities ({}); at most \
                     {MAX_OWNER_CHAIN} are allowed",
                    entity.entity_name, chain.len(), chain.join(" -> "),
                ));
            }
            // Anything on the chain but the first link is resolved by a generated
            // function whose name carries both identifiers. Truncation at Postgres's
            // 63-byte limit would collide two entities into one resolver, silently
            // handing one entity's rows the other's owners, so refuse the name here.
            let resolver = super::owner_resolver_name(&manifest.app_id, parent);
            if !super::fits_ident_limit(&resolver) {
                return Err(format!(
                    "entity '{parent}' of app '{}' needs an ownership resolver named \
                     '{resolver}', which exceeds PostgreSQL's 63-byte identifier limit; \
                     shorten the app or entity name",
                    manifest.app_id,
                ));
            }

            // The reference itself is validated with every other `entity_link`, so a
            // missing target cannot reach here.
            let target = by_name[parent];
            if owner_field(target).is_none() {
                return Err(format!(
                    "entity '{}' delegates ownership to '{parent}', which declares no owner \
                     field; mark the column that owns a '{parent}' row with \"owner\": true",
                    entity.entity_name,
                ));
            }
            match owner_parent(target) {
                Some(next) => parent = next,
                None => break,
            }
        }
    }
    Ok(())
}

pub(crate) fn validate_owner_field(entity: &EntityContract) -> Result<(), String> {
    let owners: Vec<&FieldContract> = entity.fields.iter().filter(|f| f.owner).collect();

    // Zero is the norm. Two would make "the owner" ambiguous, and silently picking
    // one decides who sees what — so refuse rather than guess.
    let [owner] = owners[..] else {
        if owners.is_empty() {
            return Ok(());
        }
        let names: Vec<&str> = owners.iter().map(|f| f.name.as_str()).collect();
        return Err(format!(
            "entity '{}' marks {} fields as owner ({}); exactly one column may own a row",
            entity.entity_name, owners.len(), names.join(", ")
        ));
    };

    // A user id is compared as-is against the caller's, so the column has to be
    // able to hold one. Anything else would build a policy matching no row, which
    // reads as an access bug rather than as the manifest mistake it is.
    if !matches!(owner.field_type.as_str(), "entity_link" | "uuid" | "text") {
        return Err(format!(
            "entity '{}': owner field '{}' is '{}'; must be entity_link, uuid or text to hold a user id",
            entity.entity_name, owner.name, owner.field_type
        ));
    }

    // Without a target there is no way to tell "holds a user id" from "defers to
    // the entity it links to", and the two generate opposite policies.
    if owner.field_type == "entity_link" && owner.references.is_none() {
        return Err(format!(
            "entity '{}': owner field '{}' is an entity_link with no 'references'; point it at \
             'core:users' to hold a user id, or at the entity that owns the row",
            entity.entity_name, owner.name,
        ));
    }
    if owner.field_type == "entity_link"
        && let Some(reference) = &owner.references
        && matches!(parse_entity_ref(&reference.entity), RefTarget::Core(_))
        && (reference.entity != "core:users" || reference.field != "id")
    {
        return Err(format!("entity '{}': direct ownership must reference core:users.id", entity.entity_name));
    }
    Ok(())
}
