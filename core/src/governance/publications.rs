use std::collections::{HashMap, HashSet};

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use rootcx_types::{AppManifest, PublicPublication};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::{api_error::ApiError, auth::identity::Identity, routes::SharedRuntime};

const MANAGEMENT_PERMISSION: &str = "admin:publications.manage";

#[derive(Debug, Clone)]
pub struct PublicExecution {
    pub consumer_app: String,
    pub installation_id: Uuid,
    pub principal_id: Uuid,
    pub publications: Vec<ApprovedPublication>,
}

impl PublicExecution {
    pub fn key(&self) -> String {
        let mut revisions: Vec<_> = self
            .publications
            .iter()
            .map(|p| (p.id, p.version))
            .collect();
        revisions.sort_unstable();
        format!(
            "public:{}:{}:{}",
            self.installation_id,
            self.principal_id,
            revisions
                .iter()
                .map(|(id, version)| format!("{id}@{version}"))
                .collect::<Vec<_>>()
                .join(",")
        )
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovedPublication {
    pub id: Uuid,
    pub version: i64,
    pub consumer_app: String,
    pub provider_app: String,
    pub consumer_installation_id: Uuid,
    pub provider_installation_id: Uuid,
    pub definition: PublicPublication,
}

#[derive(sqlx::FromRow)]
struct Snapshot {
    id: Uuid,
    version: i64,
    consumer_app: String,
    provider_app: String,
    consumer_installation_id: Uuid,
    provider_installation_id: Uuid,
    definition: Value,
}

impl TryFrom<Snapshot> for ApprovedPublication {
    type Error = ApiError;

    fn try_from(row: Snapshot) -> Result<Self, Self::Error> {
        Ok(Self {
            id: row.id,
            version: row.version,
            consumer_app: row.consumer_app,
            provider_app: row.provider_app,
            consumer_installation_id: row.consumer_installation_id,
            provider_installation_id: row.provider_installation_id,
            definition: serde_json::from_value(row.definition)
                .map_err(|_| ApiError::Forbidden("invalid publication snapshot".into()))?,
        })
    }
}

fn identifier(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 63
        || !value
            .bytes()
            .enumerate()
            .all(|(i, b)| b == b'_' || b.is_ascii_lowercase() || (i > 0 && b.is_ascii_digit()))
    {
        return Err(format!("invalid publication identifier '{value}'"));
    }
    Ok(())
}

fn validate_definition(definition: &PublicPublication) -> Result<(), String> {
    identifier(&definition.name)?;
    identifier(&definition.entity)?;
    if let Some(app) = &definition.app {
        if !app.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
            || !app
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
        {
            return Err(format!("invalid publication app identifier '{app}'"));
        }
    }
    if definition.actions.is_empty()
        || definition
            .actions
            .iter()
            .any(|action| !matches!(action.as_str(), "list" | "read"))
    {
        return Err("publication actions must be a nonempty list of list/read".into());
    }
    if definition.fields.is_empty() {
        return Err("publication fields must be explicit and nonempty".into());
    }
    for field in &definition.fields {
        identifier(field)?;
    }
    if definition.fields.iter().collect::<HashSet<_>>().len() != definition.fields.len()
        || definition.actions.iter().collect::<HashSet<_>>().len() != definition.actions.len()
    {
        return Err("duplicate publication field or action".into());
    }
    if !definition.where_clause.is_object() {
        return Err("publication where must be an object".into());
    }
    Ok(())
}

fn validate_provider(definition: &PublicPublication, provider: &AppManifest) -> Result<(), String> {
    let entity = provider
        .data_contract
        .iter()
        .find(|e| e.entity_name == definition.entity)
        .ok_or_else(|| format!("unknown publication entity '{}'", definition.entity))?;
    let types = crate::data_types::field_types(entity)?;
    let filter_fields = super::cross_app::query_field_names(Some(&definition.where_clause), None);
    for name in definition.fields.iter().chain(filter_fields.iter()) {
        let field = types
            .get(name)
            .ok_or_else(|| format!("unknown publication field '{name}'"))?;
        if field.sensitive || field.field_type.is_json() {
            return Err(format!(
                "publication field '{name}' cannot be sensitive or json"
            ));
        }
    }
    validate_filter(&definition.where_clause, &types)?;
    crate::routes::crud::build_where_clause(
        &definition.where_clause,
        &types,
        &mut Vec::new(),
        &mut 0,
    )
    .map_err(|error| format!("invalid publication where: {error:?}"))?;
    Ok(())
}

fn validate_filter(clause: &Value, types: &crate::data_types::FieldTypes) -> Result<(), String> {
    use crate::data_types::FieldType;
    let object = clause
        .as_object()
        .ok_or("publication where must be an object")?;
    for (name, value) in object {
        match name.as_str() {
            "$and" | "$or" => {
                for item in value.as_array().ok_or("logical filter must be an array")? {
                    validate_filter(item, types)?;
                }
                continue;
            }
            "$not" => {
                validate_filter(value, types)?;
                continue;
            }
            _ => {}
        }
        let field = types
            .get(name)
            .ok_or_else(|| format!("unknown publication filter '{name}'"))?;
        let Some(operators) = value.as_object() else {
            validate_filter_value(name, &field.field_type, value)?;
            continue;
        };
        if operators.is_empty() {
            return Err("publication filter operator object cannot be empty".into());
        }
        for (operator, operand) in operators {
            if operand.is_null() && !matches!(operator.as_str(), "$eq" | "$ne") {
                return Err("null publication filter requires $eq or $ne".into());
            }
            match operator.as_str() {
                "$isNull" if operand.is_boolean() => {}
                "$isNull" => return Err("$isNull must be a boolean".into()),
                "$contains" => {
                    let values = operand.as_array().ok_or("$contains must be an array")?;
                    if values.is_empty() {
                        return Err("$contains must be nonempty".into());
                    }
                    let valid = match field.field_type {
                        FieldType::TextArray => values.iter().all(Value::is_string),
                        FieldType::NumberArray => values.iter().all(Value::is_number),
                        _ => false,
                    };
                    if !valid {
                        return Err(format!("invalid array publication filter for '{name}'"));
                    }
                }
                "$in" | "$nin" => {
                    for item in operand
                        .as_array()
                        .ok_or("membership filter must be an array")?
                    {
                        if item.is_null() {
                            return Err("membership filter values cannot be null".into());
                        }
                        validate_filter_value(name, &field.field_type, item)?;
                    }
                }
                "$like" | "$ilike"
                    if !matches!(field.field_type, FieldType::Text | FieldType::File) =>
                {
                    return Err("pattern filters require text fields".into());
                }
                _ => validate_filter_value(name, &field.field_type, operand)?,
            }
        }
    }
    Ok(())
}

fn validate_filter_value(
    name: &str,
    field_type: &crate::data_types::FieldType,
    value: &Value,
) -> Result<(), String> {
    use crate::data_types::FieldType;

    if value.is_null() {
        return Ok(());
    }
    let valid = match field_type {
        FieldType::Text | FieldType::File => value.is_string(),
        FieldType::Number => value.is_number(),
        FieldType::Boolean => value.is_boolean(),
        FieldType::Decimal(_) => value
            .as_str()
            .is_some_and(|v| v.parse::<sqlx::types::BigDecimal>().is_ok()),
        FieldType::Uuid | FieldType::EntityLink => {
            value.as_str().is_some_and(|v| v.parse::<Uuid>().is_ok())
        }
        FieldType::Date => value
            .as_str()
            .is_some_and(|v| v.parse::<chrono::NaiveDate>().is_ok()),
        FieldType::Timestamp => value
            .as_str()
            .is_some_and(|v| v.parse::<DateTime<Utc>>().is_ok()),
        _ => false,
    };
    if !valid {
        return Err(format!("invalid typed publication filter for '{name}'"));
    }
    Ok(())
}

fn validate_profile(consumer: &str, definitions: &[&PublicPublication]) -> Result<(), String> {
    let mut scope = HashSet::new();
    for definition in definitions {
        for action in &definition.actions {
            if !scope.insert((
                definition.app.as_deref().unwrap_or(consumer),
                definition.entity.as_str(),
                action.as_str(),
            )) {
                return Err(
                    "ambiguous publication profile for the same provider/entity/action".into(),
                );
            }
        }
    }
    Ok(())
}

pub fn validate_manifest(manifest: &AppManifest) -> Result<(), String> {
    let Some(surface) = &manifest.public else {
        return Ok(());
    };
    let mut definitions = HashMap::new();
    for definition in &surface.publications {
        validate_definition(definition)?;
        if definitions
            .insert(definition.name.as_str(), definition)
            .is_some()
        {
            return Err(format!("duplicate publication '{}'", definition.name));
        }
        if definition.app.as_deref().unwrap_or(&manifest.app_id) == manifest.app_id {
            validate_provider(definition, manifest)?;
        }
    }
    let mut rpcs = HashSet::new();
    for rpc in &surface.rpcs {
        if !rpc.scope.is_empty() && !rpc.publications.is_empty() {
            return Err(format!(
                "public RPC '{}' cannot combine scope and publications",
                rpc.name
            ));
        }
        if !rpcs.insert(&rpc.name) {
            return Err(format!("duplicate public RPC '{}'", rpc.name));
        }
        let mut names = HashSet::new();
        let mut profile = Vec::new();
        for name in &rpc.publications {
            if !names.insert(name) {
                return Err(format!("duplicate publication reference '{name}'"));
            }
            profile.push(
                *definitions
                    .get(name.as_str())
                    .ok_or_else(|| format!("unknown publication reference '{name}'"))?,
            );
        }
        validate_profile(&manifest.app_id, &profile)?;
    }
    let mut collections = HashSet::new();
    for collection in &surface.collections {
        if !collections.insert(&collection.entity) {
            return Err(format!(
                "duplicate public collection '{}'",
                collection.entity
            ));
        }
        if let Some(name) = &collection.publication {
            let definition = definitions
                .get(name.as_str())
                .ok_or_else(|| format!("unknown publication reference '{name}'"))?;
            if definition.entity != collection.entity
                || collection.actions.is_empty()
                || collection
                    .actions
                    .iter()
                    .any(|action| !definition.actions.contains(action))
            {
                return Err(format!(
                    "public collection '{}' exceeds its publication",
                    collection.entity
                ));
            }
        }
    }
    Ok(())
}

pub async fn register_permissions(pool: &PgPool, app_id: &str) -> Result<(), crate::RuntimeError> {
    sqlx::query(
        "INSERT INTO rootcx_system.rbac_permissions (key, description, source_app)
         VALUES ($1, 'Approve, inspect and revoke public data publications', NULL),
                ($2, 'Approve public data from this app', $3)
         ON CONFLICT (key) DO NOTHING",
    )
    .bind(MANAGEMENT_PERMISSION)
    .bind(format!("app:{app_id}:publications.approve"))
    .bind(app_id)
    .execute(pool)
    .await
    .map_err(crate::RuntimeError::Schema)?;
    Ok(())
}

async fn manifest_tx(
    tx: &mut Transaction<'_, Postgres>,
    app: &str,
) -> Result<AppManifest, ApiError> {
    let value: Value = sqlx::query_scalar(
        "SELECT manifest FROM rootcx_system.apps WHERE id = $1 AND status IN ('installed', 'system')",
    ).bind(app).fetch_optional(&mut **tx).await?
        .ok_or_else(|| ApiError::NotFound(format!("active app '{app}' not found")))?;
    serde_json::from_value(value)
        .map_err(|_| ApiError::Forbidden("invalid installed manifest".into()))
}

fn declaration<'a>(
    manifest: &'a AppManifest,
    name: &str,
) -> Result<&'a PublicPublication, ApiError> {
    manifest
        .public
        .as_ref()
        .and_then(|surface| {
            surface
                .publications
                .iter()
                .find(|definition| definition.name == name)
        })
        .ok_or_else(|| ApiError::NotFound(format!("publication '{name}' is not declared")))
}

async fn installation_tx(tx: &mut Transaction<'_, Postgres>, app: &str) -> Result<Uuid, ApiError> {
    sqlx::query_scalar(
        "SELECT id FROM rootcx_system.app_installations WHERE app_id = $1 AND active FOR SHARE",
    )
    .bind(app)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ApiError::Conflict(format!("app '{app}' has no active installation")))
}

// Installations always precede publication locks, matching lifecycle UPDATEs.
async fn lock_installations(
    tx: &mut Transaction<'_, Postgres>,
    consumer: &str,
    provider: &str,
) -> Result<(Uuid, Uuid), ApiError> {
    if consumer <= provider {
        let consumer_id = installation_tx(tx, consumer).await?;
        let provider_id = if consumer == provider {
            consumer_id
        } else {
            installation_tx(tx, provider).await?
        };
        Ok((consumer_id, provider_id))
    } else {
        let provider_id = installation_tx(tx, provider).await?;
        Ok((installation_tx(tx, consumer).await?, provider_id))
    }
}

async fn exclusive_lock(tx: &mut Transaction<'_, Postgres>, key: &str) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT pg_advisory_xact_lock(43, hashtext($1))")
        .bind(key)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub async fn lock_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    publication: &ApprovedPublication,
) -> Result<(), sqlx::Error> {
    let check = async {
        let ids =
            lock_installations(tx, &publication.consumer_app, &publication.provider_app).await?;
        if ids
            != (
                publication.consumer_installation_id,
                publication.provider_installation_id,
            )
        {
            return Err(ApiError::Forbidden(
                "publication installation changed".into(),
            ));
        }
        sqlx::query("SELECT pg_advisory_xact_lock_shared(43, hashtext($1))")
            .bind(publication.id.to_string())
            .execute(&mut **tx)
            .await?;
        validate_locked_publication(tx, publication).await
    }
    .await;
    check.map_err(|error| match error {
        ApiError::Internal(message) => sqlx::Error::Protocol(message),
        other => sqlx::Error::Protocol(format!("publication denied: {other:?}")),
    })
}

async fn validate_locked_publication(
    tx: &mut Transaction<'_, Postgres>,
    publication: &ApprovedPublication,
) -> Result<(), ApiError> {
    let valid: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM rootcx_system.publications
             WHERE id = $1 AND version = $2 AND status = 'active'
               AND consumer_app = $3 AND provider_app = $4
               AND consumer_installation_id = $5 AND provider_installation_id = $6
               AND definition = $7 AND (expires_at IS NULL OR expires_at > statement_timestamp()))",
    )
    .bind(publication.id)
    .bind(publication.version)
    .bind(&publication.consumer_app)
    .bind(&publication.provider_app)
    .bind(publication.consumer_installation_id)
    .bind(publication.provider_installation_id)
    .bind(json!(publication.definition))
    .fetch_one(&mut **tx)
    .await?;
    let consumer = manifest_tx(tx, &publication.consumer_app).await?;
    validate_manifest(&consumer).map_err(ApiError::Forbidden)?;
    if !valid || declaration(&consumer, &publication.definition.name)? != &publication.definition {
        return Err(ApiError::Forbidden(
            "publication authority is no longer current".into(),
        ));
    }
    let provider = manifest_tx(tx, &publication.provider_app).await?;
    validate_provider(&publication.definition, &provider).map_err(ApiError::Forbidden)
}

pub async fn resolve(
    pool: &PgPool,
    consumer_app: &str,
    names: &[String],
) -> Result<PublicExecution, ApiError> {
    let mut tx = pool.begin().await?;
    let consumer = manifest_tx(&mut tx, consumer_app).await?;
    validate_manifest(&consumer).map_err(ApiError::Forbidden)?;
    let mut unique = HashSet::new();
    let mut definitions = Vec::new();
    for name in names {
        if !unique.insert(name) {
            return Err(ApiError::Forbidden(
                "duplicate publication reference".into(),
            ));
        }
        definitions.push(declaration(&consumer, name)?);
    }
    validate_profile(consumer_app, &definitions).map_err(ApiError::Forbidden)?;
    // Resolve the complete profile in deterministic order before any authority lock.
    let mut apps: Vec<_> = definitions
        .iter()
        .map(|d| d.app.as_deref().unwrap_or(consumer_app))
        .chain(std::iter::once(consumer_app))
        .collect();
    apps.sort_unstable();
    apps.dedup();
    let mut installations = HashMap::new();
    for app in apps {
        installations.insert(app, installation_tx(&mut tx, app).await?);
    }
    let installation_id = installations[consumer_app];
    let current = manifest_tx(&mut tx, consumer_app).await?;
    for definition in &definitions {
        if declaration(&current, &definition.name)? != *definition {
            return Err(ApiError::Forbidden(
                "publication profile changed while resolving".into(),
            ));
        }
    }
    let mut publications = Vec::new();
    let mut sorted_names = names.to_vec();
    sorted_names.sort_unstable();
    for name in sorted_names {
        let row = sqlx::query_as::<_, Snapshot>(
            "SELECT id, version, consumer_app, provider_app, consumer_installation_id,
                    provider_installation_id, definition FROM rootcx_system.publications
             WHERE consumer_installation_id = $1 AND name = $2 AND status = 'active'
               AND (expires_at IS NULL OR expires_at > statement_timestamp())",
        )
        .bind(installation_id)
        .bind(&name)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| ApiError::Forbidden(format!("publication '{name}' requires approval")))?;
        let publication = ApprovedPublication::try_from(row)?;
        lock_in_tx(&mut tx, &publication).await.map_err(|_| {
            ApiError::Forbidden(format!("publication '{name}' is no longer current"))
        })?;
        publications.push(publication);
    }
    let principal_id = principal_tx(&mut tx, installation_id).await?;
    tx.commit().await?;
    Ok(PublicExecution {
        consumer_app: consumer_app.into(),
        installation_id,
        principal_id,
        publications,
    })
}

async fn principal_tx(
    tx: &mut Transaction<'_, Postgres>,
    installation_id: Uuid,
) -> Result<Uuid, ApiError> {
    exclusive_lock(tx, &format!("principal:{installation_id}")).await?;
    let existing: Option<Uuid> = sqlx::query_scalar(
        "SELECT user_id FROM rootcx_system.public_execution_principals WHERE installation_id = $1",
    )
    .bind(installation_id)
    .fetch_optional(&mut **tx)
    .await?;
    let id = if let Some(id) = existing {
        id
    } else {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO rootcx_system.users (id, email, display_name, kind, is_system)
             VALUES ($1, $2, 'Managed public execution', 'service', true)",
        )
        .bind(id)
        .bind(format!("public+{installation_id}@localhost"))
        .execute(&mut **tx)
        .await?;
        sqlx::query(
            "INSERT INTO rootcx_system.public_execution_principals (installation_id, user_id) VALUES ($1, $2)",
        ).bind(installation_id).bind(id).execute(&mut **tx).await?;
        id
    };
    let valid: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM rootcx_system.users
         WHERE id = $1 AND kind = 'service' AND is_system
           AND disabled_at IS NULL)",
    )
    .bind(id)
    .fetch_one(&mut **tx)
    .await?;
    if !valid {
        return Err(ApiError::Forbidden(
            "public execution principal is disabled or invalid".into(),
        ));
    }
    Ok(id)
}

async fn can_manage_tx(
    tx: &mut Transaction<'_, Postgres>,
    actor: Uuid,
    provider: &str,
) -> Result<bool, ApiError> {
    Ok(sqlx::query_scalar(
        "SELECT rootcx_system.has_permission($1, $2) OR rootcx_system.has_permission($1, $3)",
    )
    .bind(actor)
    .bind(MANAGEMENT_PERMISSION)
    .bind(format!("app:{provider}:publications.approve"))
    .fetch_one(&mut **tx)
    .await?)
}

async fn require_provider(
    tx: &mut Transaction<'_, Postgres>,
    actor: Uuid,
    provider: &str,
) -> Result<(), ApiError> {
    if can_manage_tx(tx, actor, provider).await? {
        Ok(())
    } else {
        Err(ApiError::Forbidden(
            "provider publication approval permission required".into(),
        ))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalRequest {
    pub consumer_installation_id: Uuid,
    pub provider_installation_id: Uuid,
    pub reason: String,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevokeRequest {
    pub reason: String,
}

fn require_reason(reason: &str) -> Result<(), ApiError> {
    if reason.trim().is_empty() {
        Err(ApiError::BadRequest("reason is required".into()))
    } else {
        Ok(())
    }
}

async fn audit_tx(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    actor: Uuid,
    operation: &str,
    reason: &str,
) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO rootcx_system.publication_audit (publication_id, actor_id, operation, reason, snapshot)
         SELECT id, $2, $3, $4, to_jsonb(p) FROM rootcx_system.publications p WHERE id = $1",
    ).bind(id).bind(actor).bind(operation).bind(reason).execute(&mut **tx).await?;
    Ok(())
}

async fn terminate_tx(
    tx: &mut Transaction<'_, Postgres>,
    id: Uuid,
    actor: Uuid,
    status: &str,
    reason: &str,
) -> Result<(), ApiError> {
    exclusive_lock(tx, &id.to_string()).await?;
    let changed = sqlx::query(
        "UPDATE rootcx_system.publications SET status = $2, version = version + 1, revoked_at = now()
         WHERE id = $1 AND status IN ('active', 'disabled')",
    ).bind(id).bind(status).execute(&mut **tx).await?;
    if changed.rows_affected() == 0 {
        return Err(ApiError::Conflict("publication is already terminal".into()));
    }
    audit_tx(tx, id, actor, status, reason).await
}

async fn approve(
    State(rt): State<SharedRuntime>,
    identity: Identity,
    Path((app_id, name)): Path<(String, String)>,
    Json(body): Json<ApprovalRequest>,
) -> Result<(StatusCode, Json<ApprovedPublication>), ApiError> {
    let pool = crate::routes::pool(&rt);
    let publication = approve_publication(&pool, identity.user_id, app_id, name, body).await?;
    Ok((StatusCode::CREATED, Json(publication)))
}

async fn approve_publication(
    pool: &PgPool,
    actor: Uuid,
    app_id: String,
    name: String,
    body: ApprovalRequest,
) -> Result<ApprovedPublication, ApiError> {
    require_reason(&body.reason)?;
    if body.expires_at.is_some_and(|expiry| expiry <= Utc::now()) {
        return Err(ApiError::BadRequest(
            "expiresAt must be in the future".into(),
        ));
    }
    let mut tx = pool.begin().await?;
    let initial = manifest_tx(&mut tx, &app_id).await?;
    let requested = declaration(&initial, &name)?.clone();
    let provider_app = requested.app.as_deref().unwrap_or(&app_id).to_owned();
    require_provider(&mut tx, actor, &provider_app).await?;
    let (consumer_id, provider_id) = lock_installations(&mut tx, &app_id, &provider_app).await?;
    if (consumer_id, provider_id) != (body.consumer_installation_id, body.provider_installation_id)
    {
        return Err(ApiError::Conflict(
            "installation IDs are stale; reload publications".into(),
        ));
    }
    let consumer = manifest_tx(&mut tx, &app_id).await?;
    validate_manifest(&consumer).map_err(ApiError::BadRequest)?;
    let definition = declaration(&consumer, &name)?;
    if definition != &requested {
        return Err(ApiError::Conflict("publication declaration changed".into()));
    }
    let provider = manifest_tx(&mut tx, &provider_app).await?;
    validate_provider(definition, &provider).map_err(ApiError::BadRequest)?;
    exclusive_lock(&mut tx, &format!("approval:{consumer_id}:{name}")).await?;
    let previous: Option<(Uuid, Uuid, Value, bool, String)> = sqlx::query_as(
        "SELECT id, provider_installation_id, definition,
                expires_at IS NOT NULL AND expires_at <= statement_timestamp(), status
         FROM rootcx_system.publications WHERE consumer_installation_id = $1 AND name = $2 AND status IN ('active', 'disabled')",
    ).bind(consumer_id).bind(&name).fetch_optional(&mut *tx).await?;
    if let Some((id, previous_provider, previous_definition, expired, status)) = previous {
        if status == "disabled" {
            return Err(ApiError::Conflict(
                "publication is disabled; explicitly enable or revoke it".into(),
            ));
        }
        if !expired && previous_provider == provider_id && previous_definition == json!(definition)
        {
            return Err(ApiError::Conflict("publication is already approved".into()));
        }
        terminate_tx(
            &mut tx,
            id,
            actor,
            if expired { "expired" } else { "revoked" },
            "superseded by a fresh approval",
        )
        .await?;
    }
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO rootcx_system.publications
         (id, name, consumer_app, provider_app, consumer_installation_id, provider_installation_id,
          definition, approved_by, reason, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(id)
    .bind(&name)
    .bind(&app_id)
    .bind(&provider_app)
    .bind(consumer_id)
    .bind(provider_id)
    .bind(json!(definition))
    .bind(actor)
    .bind(&body.reason)
    .bind(body.expires_at)
    .execute(&mut *tx)
    .await?;
    audit_tx(&mut tx, id, actor, "approved", &body.reason).await?;
    let publication = ApprovedPublication {
        id,
        version: 1,
        consumer_app: app_id,
        provider_app,
        consumer_installation_id: consumer_id,
        provider_installation_id: provider_id,
        definition: definition.clone(),
    };
    tx.commit().await?;
    Ok(publication)
}

async fn revoke(
    State(rt): State<SharedRuntime>,
    identity: Identity,
    Path((app_id, name)): Path<(String, String)>,
    Json(body): Json<RevokeRequest>,
) -> Result<Json<Value>, ApiError> {
    let pool = crate::routes::pool(&rt);
    transition_publication(
        &pool,
        identity.user_id,
        &app_id,
        &name,
        Transition::Revoke,
        &body.reason,
    )
    .await
    .map(Json)
}

async fn disable(
    State(rt): State<SharedRuntime>,
    identity: Identity,
    Path((app_id, name)): Path<(String, String)>,
    Json(body): Json<RevokeRequest>,
) -> Result<Json<Value>, ApiError> {
    let pool = crate::routes::pool(&rt);
    transition_publication(
        &pool,
        identity.user_id,
        &app_id,
        &name,
        Transition::Disable,
        &body.reason,
    )
    .await
    .map(Json)
}

async fn enable(
    State(rt): State<SharedRuntime>,
    identity: Identity,
    Path((app_id, name)): Path<(String, String)>,
    Json(body): Json<RevokeRequest>,
) -> Result<Json<Value>, ApiError> {
    let pool = crate::routes::pool(&rt);
    transition_publication(
        &pool,
        identity.user_id,
        &app_id,
        &name,
        Transition::Enable,
        &body.reason,
    )
    .await
    .map(Json)
}

#[derive(Clone, Copy)]
enum Transition {
    Disable,
    Enable,
    Revoke,
}

async fn transition_publication(
    pool: &PgPool,
    actor: Uuid,
    app_id: &str,
    name: &str,
    transition: Transition,
    reason: &str,
) -> Result<Value, ApiError> {
    require_reason(reason)?;
    let mut tx = pool.begin().await?;
    let row = sqlx::query_as::<_, Snapshot>(
        "SELECT id, version, consumer_app, provider_app, consumer_installation_id,
                provider_installation_id, definition FROM rootcx_system.publications
         WHERE consumer_app = $1 AND name = $2 AND status IN ('active', 'disabled')
         ORDER BY created_at DESC, id DESC LIMIT 1",
    )
    .bind(app_id)
    .bind(&name)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| ApiError::Conflict("publication has no nonterminal approval".into()))?;
    let mut publication = ApprovedPublication::try_from(row)?;
    require_provider(&mut tx, actor, &publication.provider_app).await?;
    // Retired installations remain lockable for withdrawal, but never for enablement.
    sqlx::query(
        "SELECT id FROM rootcx_system.app_installations WHERE id = ANY($1)
         ORDER BY app_id FOR SHARE",
    )
    .bind(vec![
        publication.consumer_installation_id,
        publication.provider_installation_id,
    ])
    .fetch_all(&mut *tx)
    .await?;
    if matches!(transition, Transition::Enable)
        && lock_installations(
            &mut tx,
            &publication.consumer_app,
            &publication.provider_app,
        )
        .await?
            != (
                publication.consumer_installation_id,
                publication.provider_installation_id,
            )
    {
        return Err(ApiError::Conflict(
            "publication installations changed; obtain a new approval".into(),
        ));
    }
    exclusive_lock(
        &mut tx,
        &format!("approval:{}:{name}", publication.consumer_installation_id),
    )
    .await?;
    exclusive_lock(&mut tx, &publication.id.to_string()).await?;
    let (status, expired, version): (String, bool, i64) = sqlx::query_as(
        "SELECT status, expires_at IS NOT NULL AND expires_at <= statement_timestamp(), version
         FROM rootcx_system.publications WHERE id = $1",
    )
    .bind(publication.id)
    .fetch_one(&mut *tx)
    .await?;
    if !matches!(status.as_str(), "active" | "disabled") {
        return Err(ApiError::Conflict("publication is already terminal".into()));
    }
    if expired {
        terminate_tx(&mut tx, publication.id, actor, "expired", reason).await?;
        tx.commit().await?;
        return Err(ApiError::Conflict("publication has expired".into()));
    }
    let (next, operation) = match (transition, status.as_str()) {
        (Transition::Disable, "active") => ("disabled", "disabled"),
        (Transition::Enable, "disabled") => ("active", "enabled"),
        (Transition::Revoke, _) => ("revoked", "revoked"),
        _ => {
            return Err(ApiError::Conflict(
                "publication is not in the required state".into(),
            ));
        }
    };
    if next == "revoked" {
        terminate_tx(&mut tx, publication.id, actor, next, reason).await?;
    } else {
        sqlx::query(
            "UPDATE rootcx_system.publications SET status = $2, version = version + 1 WHERE id = $1",
        ).bind(publication.id).bind(next).execute(&mut *tx).await?;
        publication.version = version + 1;
        if matches!(transition, Transition::Enable) {
            validate_locked_publication(&mut tx, &publication)
                .await
                .map_err(|_| {
                    ApiError::Conflict(
                    "publication installation or frozen declaration changed; obtain a new approval"
                        .into(),
                )
                })?;
        }
        audit_tx(&mut tx, publication.id, actor, operation, reason).await?;
    }
    tx.commit().await?;
    Ok(json!({"id": publication.id, "version": version + 1, "status": next}))
}

async fn audit(
    State(rt): State<SharedRuntime>,
    identity: Identity,
    Path((app_id, name)): Path<(String, String)>,
) -> Result<Json<Vec<Value>>, ApiError> {
    publication_history(&crate::routes::pool(&rt), identity.user_id, &app_id, &name)
        .await
        .map(Json)
}

async fn publication_history(
    pool: &PgPool,
    actor: Uuid,
    app_id: &str,
    name: &str,
) -> Result<Vec<Value>, ApiError> {
    let mut tx = pool.begin().await?;
    let allowed: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM rootcx_system.publications p
         WHERE consumer_app = $1 AND name = $2 AND (
             rootcx_system.has_permission($3, $4)
             OR rootcx_system.has_permission($3, 'app:' || provider_app || ':publications.approve')))",
    ).bind(app_id).bind(name).bind(actor).bind(MANAGEMENT_PERMISSION).fetch_one(&mut *tx).await?;
    if !allowed {
        let manifest = manifest_tx(&mut tx, app_id).await?;
        let definition = declaration(&manifest, name)?;
        require_provider(&mut tx, actor, definition.app.as_deref().unwrap_or(app_id)).await?;
    }
    let rows = sqlx::query_scalar(
        "SELECT jsonb_build_object(
             'id', a.id, 'publicationId', a.publication_id, 'actorId', a.actor_id,
             'operation', a.operation, 'reason', a.reason, 'snapshot', a.snapshot, 'createdAt', a.created_at)
         FROM rootcx_system.publication_audit a
         JOIN rootcx_system.publications p ON p.id = a.publication_id
         WHERE p.consumer_app = $1 AND p.name = $2 AND (
             rootcx_system.has_permission($3, $4)
             OR rootcx_system.has_permission($3, 'app:' || p.provider_app || ':publications.approve'))
         ORDER BY a.created_at DESC, a.id DESC LIMIT 1000",
    ).bind(app_id).bind(name).bind(actor).bind(MANAGEMENT_PERMISSION).fetch_all(&mut *tx).await?;
    tx.commit().await?;
    Ok(rows)
}

async fn list(
    State(rt): State<SharedRuntime>,
    identity: Identity,
    Path(app_id): Path<String>,
) -> Result<Json<Vec<Value>>, ApiError> {
    let pool = crate::routes::pool(&rt);
    let mut tx = pool.begin().await?;
    let can_view_all = can_manage_tx(&mut tx, identity.user_id, &app_id).await?;
    let consumer = manifest_tx(&mut tx, &app_id).await?;
    let consumer_id = installation_tx(&mut tx, &app_id).await?;
    let mut rows = Vec::new();
    for definition in consumer
        .public
        .iter()
        .flat_map(|surface| &surface.publications)
    {
        let provider = definition.app.as_deref().unwrap_or(&app_id);
        if !can_view_all && !can_manage_tx(&mut tx, identity.user_id, provider).await? {
            continue;
        }
        let provider_id: Option<Uuid> = sqlx::query_scalar(
            "SELECT i.id FROM rootcx_system.app_installations i
             JOIN rootcx_system.apps a ON a.id = i.app_id
             WHERE i.app_id = $1 AND i.active AND a.status IN ('installed', 'system')",
        )
        .bind(provider)
        .fetch_optional(&mut *tx)
        .await?;
        let approval: Option<Value> = sqlx::query_scalar(
            "SELECT to_jsonb(p) FROM rootcx_system.publications p
             WHERE consumer_installation_id = $1 AND name = $2 ORDER BY created_at DESC, id LIMIT 1",
        ).bind(consumer_id).bind(&definition.name).fetch_optional(&mut *tx).await?;
        let status = match &approval {
            Some(row) if row["status"] == "revoked" => "revoked",
            Some(row)
                if row["status"] == "expired"
                    || row["expires_at"]
                        .as_str()
                        .and_then(|s| s.parse::<DateTime<Utc>>().ok())
                        .is_some_and(|expiry| expiry <= Utc::now()) =>
            {
                "expired"
            }
            Some(row) if row["status"] == "disabled" => "disabled",
            Some(row)
                if row["definition"] == json!(definition)
                    && row["provider_installation_id"] == json!(provider_id) =>
            {
                "active"
            }
            _ => "pending",
        };
        rows.push(json!({
            "name": definition.name, "definition": definition, "consumerApp": app_id,
            "providerApp": provider, "consumerInstallationId": consumer_id,
            "providerInstallationId": provider_id, "status": status, "approval": approval,
        }));
    }
    if !can_view_all && rows.is_empty() {
        return Err(ApiError::Forbidden(
            "provider publication approval permission required".into(),
        ));
    }
    tx.commit().await?;
    Ok(Json(rows))
}

pub fn routes() -> Router<SharedRuntime> {
    Router::new()
        .route("/api/v1/apps/{app_id}/publications", get(list))
        .route(
            "/api/v1/apps/{app_id}/publications/{name}/approve",
            post(approve),
        )
        .route(
            "/api/v1/apps/{app_id}/publications/{name}/revoke",
            post(revoke),
        )
        .route(
            "/api/v1/apps/{app_id}/publications/{name}/disable",
            post(disable),
        )
        .route(
            "/api/v1/apps/{app_id}/publications/{name}/enable",
            post(enable),
        )
        .route(
            "/api/v1/apps/{app_id}/publications/{name}/audit",
            get(audit),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> AppManifest {
        serde_json::from_value(json!({
            "appId": "catalog", "name": "Catalog",
            "dataContract": [{"entityName": "products", "fields": [
                {"name": "title", "type": "text"},
                {"name": "price", "type": "number"},
                {"name": "tags", "type": "[text]"},
                {"name": "secret", "type": "text", "sensitive": true},
                {"name": "metadata", "type": "json"}
            ]}],
            "public": {
                "publications": [{"name": "browse", "entity": "products", "fields": ["id", "title"]}],
                "rpcs": [{"name": "browse", "publications": ["browse"]}]
            }
        })).unwrap()
    }

    #[test]
    fn publication_scope_rejects_unsafe_fields_actions_and_filters() {
        let base = manifest();
        assert!(validate_manifest(&base).is_ok());
        for patch in [
            json!({"fields": []}),
            json!({"fields": ["secret"]}),
            json!({"fields": ["metadata"]}),
            json!({"fields": ["missing"]}),
            json!({"fields": ["id", "id"]}),
            json!({"actions": []}),
            json!({"actions": ["update"]}),
            json!({"actions": ["read", "read"]}),
            json!({"where": null}),
            json!({"where": {"secret": "x"}}),
            json!({"where": {"metadata": {"$eq": "x"}}}),
            json!({"where": {"$or": [{"missing": "x"}]}}),
            json!({"where": {"$bogus": "x"}}),
            json!({"where": {"title": {"$execute": "x"}}}),
            json!({"where": {"id": "not-a-uuid"}}),
            json!({"where": {"price": "not-a-number"}}),
            json!({"where": {"title": {"$isNull": "false"}}}),
            json!({"where": {"title": {}}}),
            json!({"where": {"$or": {}}}),
            json!({"where": {"$and": [null]}}),
            json!({"where": {"$not": []}}),
            json!({"where": {"title": {"$in": "x"}}}),
            json!({"where": {"title": {"$in": [null]}}}),
            json!({"where": {"price": {"$in": [1, "2"]}}}),
            json!({"where": {"price": {"$like": "1%"}}}),
            json!({"where": {"price": {"$gt": null}}}),
            json!({"where": {"tags": {"$contains": []}}}),
            json!({"where": {"tags": {"$contains": "x"}}}),
            json!({"where": {"tags": {"$contains": [1]}}}),
            json!({"where": {"title": {"$contains": ["x"]}}}),
        ] {
            let mut value = json!(base);
            value["public"]["publications"][0]
                .as_object_mut()
                .unwrap()
                .extend(patch.as_object().unwrap().clone());
            let candidate: AppManifest = serde_json::from_value(value).unwrap();
            assert!(validate_manifest(&candidate).is_err(), "{patch}");
        }
    }

    #[test]
    fn publication_filters_allow_composed_typed_predicates() {
        for filter in [
            json!({}),
            json!({"$and": [{"price": {"$gte": 0}}, {"$or": [
                {"title": {"$ilike": "a%"}}, {"title": {"$eq": null}}
            ]}]}),
            json!({"$not": {"title": {"$in": ["draft", "private"]}}}),
            json!({"title": {"$isNull": false}}),
            json!({"tags": {"$contains": ["public"]}}),
            json!({"price": {"$in": []}}),
        ] {
            let mut manifest = manifest();
            manifest.public.as_mut().unwrap().publications[0].where_clause = filter.clone();
            assert!(validate_manifest(&manifest).is_ok(), "{filter}");
        }
    }

    #[test]
    fn publication_identifiers_obey_the_sql_identifier_boundary() {
        for (name, valid) in [
            ("".to_string(), false),
            ("a".repeat(63), true),
            ("a".repeat(64), false),
            ("1catalog".into(), false),
            ("Catalog".into(), false),
            ("cat-alog".into(), false),
            ("cat\"alog".into(), false),
            ("catalog_2".into(), true),
        ] {
            for field in ["name", "entity", "fields"] {
                let mut definition = manifest().public.unwrap().publications.remove(0);
                match field {
                    "name" => definition.name = name.clone(),
                    "entity" => definition.entity = name.clone(),
                    _ => definition.fields = vec![name.clone()],
                }
                assert_eq!(validate_definition(&definition).is_ok(), valid, "{field}={name:?}");
            }
        }
    }

    #[test]
    fn public_collection_bindings_cannot_exceed_the_named_publication() {
        for (binding, valid) in [
            (json!({"entity": "products", "actions": ["read"], "publication": "browse"}), true),
            (json!({"entity": "products", "actions": [], "publication": "browse"}), false),
            (json!({"entity": "products", "actions": ["create"], "publication": "browse"}), false),
            (json!({"entity": "other", "actions": ["read"], "publication": "browse"}), false),
            (json!({"entity": "products", "actions": ["read"], "publication": "missing"}), false),
        ] {
            let mut value = json!(manifest());
            value["public"]["collections"] = json!([binding]);
            assert_eq!(
                validate_manifest(&serde_json::from_value(value.clone()).unwrap()).is_ok(),
                valid, "{value}"
            );
        }
    }

    #[test]
    fn publication_provider_app_ids_accept_hyphens_but_reject_unsafe_names() {
        for (app, valid) in [
            ("provider-app", true),
            ("provider_app2", true),
            ("a", true),
            ("", false),
            ("-app", false),
            ("_app", false),
            ("1app", false),
            ("App", false),
            ("app/path", false),
            ("app.name", false),
            ("app;drop", false),
        ] {
            let mut manifest = manifest();
            manifest.public.as_mut().unwrap().publications[0].app = Some(app.into());
            assert_eq!(validate_manifest(&manifest).is_ok(), valid, "{app:?}");
        }
    }

    #[test]
    fn scoped_rpcs_cannot_also_bind_publications() {
        for (scope, publications, valid) in [
            (json!([]), json!(["browse"]), true),
            (json!(["board_id"]), json!([]), true),
            (json!(["board_id"]), json!(["browse"]), false),
        ] {
            let mut value = json!(manifest());
            value["public"]["rpcs"][0]["scope"] = scope;
            value["public"]["rpcs"][0]["publications"] = publications;
            assert_eq!(
                validate_manifest(&serde_json::from_value(value.clone()).unwrap()).is_ok(),
                valid,
                "{value}"
            );
        }
    }

    #[test]
    fn profiles_reject_missing_duplicate_and_ambiguous_authority() {
        for names in [
            json!(["missing"]),
            json!(["browse", "browse"]),
            json!(["browse", "other"]),
        ] {
            let mut value = json!(manifest());
            let mut other = value["public"]["publications"][0].clone();
            other["name"] = json!("other");
            value["public"]["publications"]
                .as_array_mut()
                .unwrap()
                .push(other);
            value["public"]["rpcs"][0]["publications"] = names.clone();
            assert!(
                validate_manifest(&serde_json::from_value(value).unwrap()).is_err(),
                "{names}"
            );
        }
        let mut value = json!(manifest());
        let duplicate = value["public"]["publications"][0].clone();
        value["public"]["publications"]
            .as_array_mut()
            .unwrap()
            .push(duplicate);
        assert!(validate_manifest(&serde_json::from_value(value).unwrap()).is_err());
    }

    async fn fixture() -> (PgPool, AppManifest, Uuid, PublicExecution) {
        let pool = crate::extensions::test_db::pool().await;
        let mut manifest = manifest();
        manifest.app_id = format!("pub_{}", Uuid::new_v4().simple());
        sqlx::query("INSERT INTO rootcx_system.apps (id, name, manifest) VALUES ($1, 'Publication test', $2)")
            .bind(&manifest.app_id).bind(json!(manifest)).execute(&pool).await.unwrap();
        let installation = super::super::cross_app::register_installation(&pool, &manifest.app_id)
            .await
            .unwrap();
        let actor = Uuid::new_v4();
        sqlx::query("INSERT INTO rootcx_system.users (id, email) VALUES ($1, $2)")
            .bind(actor)
            .bind(format!("{actor}@test.invalid"))
            .execute(&pool)
            .await
            .unwrap();
        let definition = manifest.public.as_ref().unwrap().publications[0].clone();
        sqlx::query(
            "INSERT INTO rootcx_system.publications
                 (id, name, consumer_app, provider_app, consumer_installation_id,
                  provider_installation_id, definition, approved_by, reason)
                 VALUES ($1, 'browse', $2, $2, $3, $3, $4, $5, 'test approval')",
        )
        .bind(Uuid::new_v4())
        .bind(&manifest.app_id)
        .bind(installation)
        .bind(json!(definition))
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
        let execution = resolve(&pool, &manifest.app_id, &["browse".into()])
            .await
            .unwrap();
        let mut tx = pool.begin().await.unwrap();
        audit_tx(
            &mut tx,
            execution.publications[0].id,
            actor,
            "approved",
            "test approval",
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        (pool, manifest, actor, execution)
    }

    async fn authorize_provider(pool: &PgPool, actor: Uuid, app: &str) {
        let role = format!("publication_lifecycle_{actor}");
        sqlx::query("INSERT INTO rootcx_system.rbac_roles (name, permissions) VALUES ($1, $2)")
            .bind(&role)
            .bind(vec![format!("app:{app}:publications.approve")])
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO rootcx_system.rbac_assignments (user_id, role) VALUES ($1, $2)")
            .bind(actor)
            .bind(role)
            .execute(pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn disable_enable_changes_the_version_without_restoring_old_capabilities() {
        let (pool, manifest, actor, execution) = fixture().await;
        authorize_provider(&pool, actor, &manifest.app_id).await;
        let mut read = pool.begin().await.unwrap();
        lock_in_tx(&mut read, &execution.publications[0])
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(100),
                transition_publication(
                    &pool,
                    actor,
                    &manifest.app_id,
                    "browse",
                    Transition::Disable,
                    "pause"
                )
            )
            .await
            .is_err(),
            "disable must wait for the in-flight read"
        );
        read.commit().await.unwrap();
        let disabled = transition_publication(
            &pool,
            actor,
            &manifest.app_id,
            "browse",
            Transition::Disable,
            "pause",
        )
        .await
        .unwrap();
        assert_eq!(
            (disabled["status"].clone(), disabled["version"].clone()),
            (json!("disabled"), json!(2))
        );
        assert!(
            resolve(&pool, &manifest.app_id, &["browse".into()])
                .await
                .is_err()
        );
        assert!(matches!(
            approve_publication(
                &pool,
                actor,
                manifest.app_id.clone(),
                "browse".into(),
                ApprovalRequest {
                    consumer_installation_id: execution.installation_id,
                    provider_installation_id: execution.installation_id,
                    reason: "must not implicitly enable".into(),
                    expires_at: None,
                }
            )
            .await,
            Err(ApiError::Conflict(_))
        ));
        let enabled = transition_publication(
            &pool,
            actor,
            &manifest.app_id,
            "browse",
            Transition::Enable,
            "resume",
        )
        .await
        .unwrap();
        assert_eq!(
            (enabled["id"].clone(), enabled["version"].clone()),
            (json!(execution.publications[0].id), json!(3))
        );
        let next = resolve(&pool, &manifest.app_id, &["browse".into()])
            .await
            .unwrap();
        assert_ne!(next.key(), execution.key());
        let mut old = pool.begin().await.unwrap();
        assert!(
            lock_in_tx(&mut old, &execution.publications[0])
                .await
                .is_err()
        );
        old.rollback().await.unwrap();
        pool.close().await;
    }

    #[tokio::test]
    async fn enable_rejects_changed_installations_declarations_and_provider_fields() {
        for change in ["installation", "declaration", "sensitivity"] {
            let (pool, mut manifest, actor, execution) = fixture().await;
            authorize_provider(&pool, actor, &manifest.app_id).await;
            transition_publication(
                &pool,
                actor,
                &manifest.app_id,
                "browse",
                Transition::Disable,
                "pause",
            )
            .await
            .unwrap();
            if change == "installation" {
                sqlx::query(
                    "UPDATE rootcx_system.app_installations SET active = false WHERE id = $1",
                )
                .bind(execution.installation_id)
                .execute(&pool)
                .await
                .unwrap();
                super::super::cross_app::register_installation(&pool, &manifest.app_id)
                    .await
                    .unwrap();
            } else {
                if change == "declaration" {
                    manifest.public.as_mut().unwrap().publications[0].fields = vec!["id".into()];
                } else {
                    manifest.data_contract[0].fields[0].sensitive = true;
                }
                sqlx::query("UPDATE rootcx_system.apps SET manifest = $2 WHERE id = $1")
                    .bind(&manifest.app_id)
                    .bind(json!(manifest))
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            assert!(
                matches!(
                    transition_publication(
                        &pool,
                        actor,
                        &manifest.app_id,
                        "browse",
                        Transition::Enable,
                        "resume"
                    )
                    .await,
                    Err(ApiError::Conflict(_))
                ),
                "{change}"
            );
            let state: (String, i64) = sqlx::query_as(
                "SELECT status, version FROM rootcx_system.publications WHERE id = $1",
            )
            .bind(execution.publications[0].id)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(
                state,
                ("disabled".into(), 2),
                "{change}: failed enable must roll back"
            );
            pool.close().await;
        }
    }

    #[tokio::test]
    async fn elapsed_disabled_publications_expire_terminally_instead_of_enabling() {
        let (pool, manifest, actor, execution) = fixture().await;
        authorize_provider(&pool, actor, &manifest.app_id).await;
        transition_publication(
            &pool,
            actor,
            &manifest.app_id,
            "browse",
            Transition::Revoke,
            "superseded fixture",
        )
        .await
        .unwrap();
        let expired_id: Uuid = sqlx::query_scalar(
            "INSERT INTO rootcx_system.publications
             (name, consumer_app, provider_app, consumer_installation_id, provider_installation_id,
              definition, approved_by, reason, status, version, created_at, expires_at)
             SELECT name, consumer_app, provider_app, consumer_installation_id, provider_installation_id,
                    definition, approved_by, reason, 'disabled', 2, now() - interval '2 seconds', now() - interval '1 second'
             FROM rootcx_system.publications WHERE id = $1 RETURNING id",
        ).bind(execution.publications[0].id).fetch_one(&pool).await.unwrap();
        assert!(matches!(
            transition_publication(
                &pool,
                actor,
                &manifest.app_id,
                "browse",
                Transition::Enable,
                "resume"
            )
            .await,
            Err(ApiError::Conflict(_))
        ));
        let state: (String, i64) =
            sqlx::query_as("SELECT status, version FROM rootcx_system.publications WHERE id = $1")
                .bind(expired_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(state, ("expired".into(), 3));
        assert!(sqlx::query("UPDATE rootcx_system.publications SET status = 'active', version = version + 1 WHERE id = $1")
            .bind(expired_id).execute(&pool).await.is_err());
        pool.close().await;
    }

    #[tokio::test]
    async fn publication_management_requires_provider_permission_and_a_reason() {
        let (pool, manifest, actor, _) = fixture().await;
        assert!(matches!(
            publication_history(&pool, actor, &manifest.app_id, "browse").await,
            Err(ApiError::Forbidden(_))
        ));
        for (name, transition) in [("disable", Transition::Disable), ("enable", Transition::Enable), ("revoke", Transition::Revoke)] {
            assert!(matches!(
                transition_publication(
                    &pool,
                    actor,
                    &manifest.app_id,
                    "browse",
                    transition,
                    "unauthorized"
                )
                .await,
                Err(ApiError::Forbidden(_))
            ), "{name}");
        }
        authorize_provider(&pool, actor, &manifest.app_id).await;
        for reason in ["", " \n "] {
            assert!(matches!(
                transition_publication(
                    &pool,
                    actor,
                    &manifest.app_id,
                    "browse",
                    Transition::Disable,
                    reason
                )
                .await,
                Err(ApiError::BadRequest(_))
            ), "reason={reason:?}");
        }
        pool.close().await;
    }

    #[tokio::test]
    async fn lifecycle_history_retains_each_reviewed_snapshot() {
        let (pool, manifest, actor, execution) = fixture().await;
        authorize_provider(&pool, actor, &manifest.app_id).await;
        transition_publication(
            &pool,
            actor,
            &manifest.app_id,
            "browse",
            Transition::Disable,
            "pause consent",
        )
        .await
        .unwrap();
        transition_publication(
            &pool,
            actor,
            &manifest.app_id,
            "browse",
            Transition::Enable,
            "renew consent",
        )
        .await
        .unwrap();
        let history = publication_history(&pool, actor, &manifest.app_id, "browse")
            .await
            .unwrap();
        assert_eq!(
            history
                .iter()
                .map(|row| row["operation"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec!["enabled", "disabled", "approved"]
        );
        assert_eq!(history[0]["reason"], "renew consent");
        for (row, version) in history.iter().zip([3, 2, 1]) {
            assert_eq!(row["snapshot"]["version"], version);
            assert_eq!(
                row["snapshot"]["definition"],
                json!(execution.publications[0].definition)
            );
            assert_eq!(row["actorId"], json!(actor));
        }
        pool.close().await;
    }

    #[tokio::test]
    async fn revoked_capabilities_wait_for_reads_and_never_revive() {
        let (pool, manifest, actor, execution) = fixture().await;
        let first_id = execution.publications[0].id;
        let names = vec!["browse".into()];
        let publication = &execution.publications[0];
        let mut read = pool.begin().await.unwrap();
        lock_in_tx(&mut read, publication).await.unwrap();
        let mut revoke = pool.begin().await.unwrap();
        sqlx::query("SET LOCAL lock_timeout = '50ms'")
            .execute(&mut *revoke)
            .await
            .unwrap();
        assert!(
            terminate_tx(&mut revoke, first_id, actor, "revoked", "test revoke")
                .await
                .is_err(),
            "revocation must wait for the in-flight read's shared authority lock"
        );
        revoke.rollback().await.unwrap();
        read.commit().await.unwrap();
        let mut revoke = pool.begin().await.unwrap();
        terminate_tx(&mut revoke, first_id, actor, "revoked", "test revoke")
            .await
            .unwrap();
        revoke.commit().await.unwrap();
        assert!(resolve(&pool, &manifest.app_id, &names).await.is_err());
        assert!(sqlx::query("UPDATE rootcx_system.publications SET status = 'active', version = version + 1 WHERE id = $1")
            .bind(first_id).execute(&pool).await.is_err(), "terminal rows cannot be reactivated");
        authorize_provider(&pool, actor, &manifest.app_id).await;
        assert!(matches!(transition_publication(
            &pool, actor, &manifest.app_id, "browse", Transition::Enable, "too late",
        ).await, Err(ApiError::Conflict(_))));
        sqlx::query(
            "INSERT INTO rootcx_system.publications
             (name, consumer_app, provider_app, consumer_installation_id,
              provider_installation_id, definition, approved_by, reason)
             SELECT name, consumer_app, provider_app, consumer_installation_id,
                    provider_installation_id, definition, approved_by, 'new approval'
             FROM rootcx_system.publications WHERE id = $1",
        )
        .bind(first_id)
        .execute(&pool)
        .await
        .unwrap();
        let next = resolve(&pool, &manifest.app_id, &names).await.unwrap();
        assert_ne!(execution.key(), next.key());
        let mut stale = pool.begin().await.unwrap();
        assert!(lock_in_tx(&mut stale, publication).await.is_err());
        stale.rollback().await.unwrap();
        pool.close().await;
    }

    #[tokio::test]
    async fn installation_retirement_waits_for_reads_and_invalidates_saved_authority() {
        let (pool, _, _, execution) = fixture().await;
        let installation = execution.installation_id;
        let mut current = pool.begin().await.unwrap();
        lock_in_tx(&mut current, &execution.publications[0])
            .await
            .unwrap();
        let mut lifecycle = pool.begin().await.unwrap();
        sqlx::query("SET LOCAL lock_timeout = '50ms'")
            .execute(&mut *lifecycle)
            .await
            .unwrap();
        assert!(
            sqlx::query("UPDATE rootcx_system.app_installations SET active = false WHERE id = $1")
                .bind(installation)
                .execute(&mut *lifecycle)
                .await
                .is_err(),
            "installation retirement must wait for in-flight authority"
        );
        lifecycle.rollback().await.unwrap();
        current.commit().await.unwrap();
        sqlx::query("UPDATE rootcx_system.app_installations SET active = false WHERE id = $1")
            .bind(installation)
            .execute(&pool)
            .await
            .unwrap();
        let mut stale = pool.begin().await.unwrap();
        assert!(
            lock_in_tx(&mut stale, &execution.publications[0])
                .await
                .is_err()
        );
        stale.rollback().await.unwrap();
        pool.close().await;
    }

    #[tokio::test]
    async fn managed_principals_are_reused_and_invalid_ones_cannot_be_replaced() {
        for disable in [true, false] {
            let (pool, manifest, _, execution) = fixture().await;
            let names = vec!["browse".into()];
            let repeated = resolve(&pool, &manifest.app_id, &names).await.unwrap();
            assert_eq!(execution.principal_id, repeated.principal_id);
            let exposed_as_human: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM rootcx_system.users WHERE id = $1 AND NOT is_system)",
            )
            .bind(execution.principal_id)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert!(
                !exposed_as_human,
                "managed services must be excluded from generic human user operations"
            );
            sqlx::query(
                "UPDATE rootcx_system.users SET disabled_at = CASE WHEN $2 THEN now() ELSE NULL END,
                 is_system = $2 WHERE id = $1",
            ).bind(execution.principal_id).bind(disable).execute(&pool).await.unwrap();
            assert!(
                resolve(&pool, &manifest.app_id, &names).await.is_err(),
                "disable={disable}"
            );
            let count: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM rootcx_system.public_execution_principals WHERE installation_id = $1",
            ).bind(execution.installation_id).fetch_one(&pool).await.unwrap();
            assert_eq!(count, 1, "disable={disable}");
            pool.close().await;
        }
    }

    #[tokio::test]
    async fn approval_requires_current_installations_and_provider_authority() {
        let (pool, provider, actor, provider_execution) = fixture().await;
        let mut consumer = provider.clone();
        consumer.app_id = format!("pub_{}", Uuid::new_v4().simple());
        let declaration = &mut consumer.public.as_mut().unwrap().publications[0];
        declaration.app = Some(provider.app_id.clone());
        declaration.release_ownership = true;
        sqlx::query(
            "INSERT INTO rootcx_system.apps (id, name, manifest) VALUES ($1, 'Consumer', $2)",
        )
        .bind(&consumer.app_id)
        .bind(json!(consumer))
        .execute(&pool)
        .await
        .unwrap();
        let consumer_id = super::super::cross_app::register_installation(&pool, &consumer.app_id)
            .await
            .unwrap();
        let role = format!("publication_test_{actor}");
        sqlx::query("INSERT INTO rootcx_system.rbac_roles (name, permissions) VALUES ($1, $2)")
            .bind(&role)
            .bind(vec![format!("app:{}:*", consumer.app_id)])
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO rootcx_system.rbac_assignments (user_id, role) VALUES ($1, $2)")
            .bind(actor)
            .bind(&role)
            .execute(&pool)
            .await
            .unwrap();
        let request = |provider_id| ApprovalRequest {
            consumer_installation_id: consumer_id,
            provider_installation_id: provider_id,
            reason: "Approve the explicit public projection and ownership release".into(),
            expires_at: None,
        };
        assert!(
            matches!(
                approve_publication(
                    &pool,
                    actor,
                    consumer.app_id.clone(),
                    "browse".into(),
                    request(provider_execution.installation_id)
                )
                .await,
                Err(ApiError::Forbidden(_))
            ),
            "consumer administration is not provider approval authority"
        );
        sqlx::query("UPDATE rootcx_system.rbac_roles SET permissions = $2 WHERE name = $1")
            .bind(&role)
            .bind(vec![format!(
                "app:{}:publications.approve",
                provider.app_id
            )])
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            matches!(
                approve_publication(
                    &pool,
                    actor,
                    consumer.app_id.clone(),
                    "browse".into(),
                    request(Uuid::new_v4())
                )
                .await,
                Err(ApiError::Conflict(_))
            ),
            "the provider permission cannot approve a stale installation"
        );
        let approval = approve_publication(
            &pool,
            actor,
            consumer.app_id.clone(),
            "browse".into(),
            request(provider_execution.installation_id),
        )
        .await
        .unwrap();
        assert_eq!(
            approval.definition,
            consumer.public.unwrap().publications[0]
        );
        assert!(sqlx::query(
            "UPDATE rootcx_system.publications SET definition = jsonb_set(definition, '{fields}', '[\"secret\"]')
             WHERE id = $1",
        ).bind(approval.id).execute(&pool).await.is_err(), "approved fields cannot be edited in place");
        pool.close().await;
    }
}
