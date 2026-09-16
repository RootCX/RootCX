//! Core-owned, collection-only authority for application-to-application CRUD.
//!
//! This module is deliberately the only place that knows how a consumer
//! installation may access a provider collection. Adapters (HTTP, workers and
//! tools) must resolve a grant here before they inspect provider metadata or
//! open a provider transaction.

use std::collections::HashSet;

use axum::Json;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sqlx::{Connection, PgConnection, PgPool, Postgres};
use uuid::Uuid;

use crate::RuntimeError;
use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::data_types::FieldTypes;
use crate::routes::SharedRuntime;

pub const ACTION_LIST: &str = "list";
pub const ACTION_READ: &str = "read";
pub const ACTION_CREATE: &str = "create";
pub const ACTION_UPDATE: &str = "update";
pub const ACTION_DELETE: &str = "delete";
const MANAGEMENT_PERMISSION: &str = "admin:cross_app.grants.manage";

/// A grant is an authority object, not a permission-string shortcut.  Its
/// `field_snapshot` is always explicit, including when an administrator chose
/// "all readable fields" in the UI.  This prevents a future manifest field from
/// silently widening an already-approved trust relationship.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct CollectionGrant {
    pub id: Uuid,
    pub consumer_app: String,
    pub provider_app: String,
    pub consumer_installation_id: Uuid,
    pub provider_installation_id: Uuid,
    pub consumer_generation: i64,
    pub provider_generation: i64,
    pub entity: String,
    pub actions: Vec<String>,
    pub field_snapshot: Vec<String>,
    pub write_field_snapshot: Vec<String>,
    pub status: String,
    pub version: i64,
    pub expires_at: Option<DateTime<Utc>>,
    pub requested_by: Uuid,
    pub approved_by: Option<Uuid>,
    pub reason: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct CrossAppGrantAudit {
    pub id: Uuid,
    pub grant_id: Uuid,
    pub operation: String,
    pub actor_id: Option<Uuid>,
    pub reason: String,
    pub before_state: Option<JsonValue>,
    pub after_state: JsonValue,
    pub created_at: DateTime<Utc>,
}

/// The result of the Core's authorization decision.  It is intentionally not
/// serializable: applications never receive a reusable grant capability.
/// The historical name is retained for adapter compatibility; `action` and
/// the independent readable/writable snapshots now represent any CRUD operation.
#[derive(Debug, Clone)]
pub struct AuthorizedRead {
    pub grant_id: Uuid,
    pub grant_version: i64,
    pub consumer_app: String,
    pub provider_app: String,
    pub consumer_installation_id: Uuid,
    pub provider_installation_id: Uuid,
    pub entity: String,
    pub action: String,
    pub field_snapshot: Vec<String>,
    pub write_field_snapshot: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateGrantRequest {
    pub consumer_app: String,
    pub provider_app: String,
    pub entity: String,
    #[serde(default = "default_actions")]
    pub actions: Vec<String>,
    /// Optional administrator convenience input.  The stored grant always
    /// contains the resolved, explicit snapshot.
    #[serde(default)]
    pub fields: Option<Vec<String>>,
    /// Required explicit writable scope for create/update. Never defaulted to
    /// all fields; system and sensitive fields cannot be approved for writing.
    #[serde(default)]
    pub write_fields: Option<Vec<String>>,
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrantReason {
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct GrantListQuery {
    pub status: Option<String>,
    /// A provider admin must scope the listing to the app they administer.
    #[serde(rename = "providerApp")]
    pub provider_app: Option<String>,
}

fn default_actions() -> Vec<String> {
    vec![ACTION_LIST.into(), ACTION_READ.into()]
}

fn validate_status(status: &str) -> Result<(), ApiError> {
    if matches!(
        status,
        "pending" | "active" | "disabled" | "revoked" | "expired"
    ) {
        Ok(())
    } else {
        Err(ApiError::BadRequest(
            "invalid cross-app grant status".into(),
        ))
    }
}

fn valid_identifier(value: &str, label: &str) -> Result<(), ApiError> {
    if !value.is_empty()
        && value.as_bytes()[0].is_ascii_lowercase()
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    {
        return Ok(());
    }
    Err(ApiError::BadRequest(format!(
        "{label} '{value}' must be snake_case"
    )))
}

fn valid_app_id(value: &str, label: &str) -> Result<(), ApiError> {
    if !value.is_empty()
        && value.as_bytes()[0].is_ascii_lowercase()
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
    {
        return Ok(());
    }
    Err(ApiError::BadRequest(format!(
        "{label} '{value}' must be a safe app identifier"
    )))
}

fn valid_action(action: &str) -> bool {
    matches!(
        action,
        ACTION_LIST | ACTION_READ | ACTION_CREATE | ACTION_UPDATE | ACTION_DELETE
    )
}

fn validate_actions(actions: &[String]) -> Result<Vec<String>, ApiError> {
    if actions.is_empty() {
        return Err(ApiError::BadRequest("actions must not be empty".into()));
    }
    let mut result = Vec::new();
    for action in actions {
        if !valid_action(action) {
            return Err(ApiError::BadRequest(
                "cross-app grants support list, read, create, update and delete only".into(),
            ));
        }
        if !result.iter().any(|existing| existing == action) {
            result.push(action.clone());
        }
    }
    result.sort_unstable();
    Ok(result)
}

fn validate_requested_fields(
    types: &FieldTypes,
    requested: Option<&[String]>,
) -> Result<Vec<String>, ApiError> {
    let fields: Vec<String> = match requested {
        Some(fields) if fields.is_empty() => {
            return Err(ApiError::BadRequest(
                "fields must not be empty when supplied".into(),
            ));
        }
        Some(fields) => fields.to_vec(),
        None => types
            .iter()
            .filter(|(_, field)| !field.sensitive)
            .map(|(name, _)| name.clone())
            .collect(),
    };

    let mut seen = HashSet::new();
    let mut result = Vec::with_capacity(fields.len());
    for field in fields {
        if !types
            .get(&field)
            .is_some_and(|definition| !definition.sensitive)
        {
            return Err(ApiError::BadRequest(format!(
                "field '{field}' is not readable"
            )));
        }
        if seen.insert(field.clone()) {
            result.push(field);
        }
    }

    // System identifiers are part of the collection contract.  They must not
    // be accidentally removed by an overly narrow administrator projection.
    for required in ["id", "created_at", "updated_at"] {
        if types.contains_key(required) && !seen.contains(required) {
            result.push(required.to_string());
        }
    }
    result.sort_unstable();
    Ok(result)
}

fn validate_write_fields(
    types: &FieldTypes,
    actions: &[String],
    requested: Option<&[String]>,
) -> Result<Vec<String>, ApiError> {
    let can_write_fields = actions
        .iter()
        .any(|action| matches!(action.as_str(), ACTION_CREATE | ACTION_UPDATE));
    if !can_write_fields {
        if requested.is_some_and(|fields| !fields.is_empty()) {
            return Err(ApiError::BadRequest(
                "writeFields requires create or update actions".into(),
            ));
        }
        return Ok(Vec::new());
    }
    let fields = requested.filter(|fields| !fields.is_empty()).ok_or_else(|| {
        ApiError::BadRequest("create/update grants require explicit nonempty writeFields".into())
    })?;
    let mut result = Vec::new();
    for name in fields {
        if crate::manifest::is_system_field(name)
            || !types.get(name).is_some_and(|field| !field.sensitive)
        {
            return Err(ApiError::BadRequest(format!("field '{name}' is not writable")));
        }
        if !result.contains(name) {
            result.push(name.clone());
        }
    }
    result.sort_unstable();
    Ok(result)
}

fn stable_not_found() -> ApiError {
    ApiError::NotFound("requested collection is not available".into())
}

// Keep lifecycle sessions bounded independently of the shared query pool.
static LIFECYCLE_CONNECTIONS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);

pub(crate) struct AppLifecycleLock {
    connection: PgConnection,
    _permit: tokio::sync::SemaphorePermit<'static>,
}

impl AppLifecycleLock {
    pub(crate) async fn begin(&mut self) -> Result<sqlx::Transaction<'_, Postgres>, sqlx::Error> {
        self.connection.begin().await
    }
}

/// Hold the same database app lock across independently committed installation
/// steps. A dedicated connection keeps extension callbacks free to use even a
/// one-connection pool. Dropping it (including cancellation) closes the session,
/// releasing the lock; a session lock must never be returned to the pool.
pub(crate) async fn lock_app_lifecycle(
    pool: &PgPool,
    app_id: &str,
) -> Result<AppLifecycleLock, RuntimeError> {
    let permit = LIFECYCLE_CONNECTIONS
        .acquire()
        .await
        .map_err(|_| RuntimeError::Capacity("application lifecycle is unavailable".into()))?;
    let mut connection = PgConnection::connect_with(&pool.connect_options())
        .await
        .map_err(RuntimeError::Database)?;
    sqlx::query("SELECT pg_advisory_lock(hashtext($1))")
        .bind(app_id)
        .execute(&mut connection)
        .await
        .map_err(RuntimeError::Schema)?;
    Ok(AppLifecycleLock {
        connection,
        _permit: permit,
    })
}

/// Register one active installation generation.  Reinstalling an app after it
/// was removed always creates a new UUID and generation; it can never revive a
/// grant tied to the previous installation.
pub async fn register_installation(pool: &PgPool, app_id: &str) -> Result<Uuid, RuntimeError> {
    let mut connection = lock_app_lifecycle(pool, app_id).await?;
    let mut tx = connection.begin().await.map_err(RuntimeError::Schema)?;
    let id = register_installation_tx(&mut tx, app_id).await?;
    tx.commit().await.map_err(RuntimeError::Schema)?;
    Ok(id)
}

pub(crate) async fn register_installation_tx(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    app_id: &str,
) -> Result<Uuid, RuntimeError> {
    // Serialize only installations of the same app.  Without this lock two
    // concurrent installs could both choose generation N+1 and one would fail
    // after the app schema was already created.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
        .bind(app_id)
        .execute(&mut **tx)
        .await
        .map_err(RuntimeError::Schema)?;
    let existing: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM rootcx_system.app_installations
         WHERE app_id = $1 AND active = TRUE
         ORDER BY generation DESC LIMIT 1",
    )
    .bind(app_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(RuntimeError::Schema)?;
    if let Some((id,)) = existing {
        return Ok(id);
    }

    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO rootcx_system.app_installations (id, app_id, generation)
         SELECT $1, $2, COALESCE(MAX(generation), 0) + 1
         FROM rootcx_system.app_installations WHERE app_id = $2",
    )
    .bind(id)
    .bind(app_id)
    .execute(&mut **tx)
    .await
    .map_err(RuntimeError::Schema)?;
    Ok(id)
}

/// Mark the current generation inactive and revoke every grant involving it.
pub async fn deactivate_installation(pool: &PgPool, app_id: &str) -> Result<(), RuntimeError> {
    let mut connection = lock_app_lifecycle(pool, app_id).await?;
    let mut tx = connection.begin().await.map_err(RuntimeError::Schema)?;
    deactivate_installation_tx(&mut tx, app_id, None, "application uninstalled").await?;
    tx.commit().await.map_err(RuntimeError::Schema)?;
    Ok(())
}

pub(crate) async fn deactivate_installation_tx(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    app_id: &str,
    actor_id: Option<Uuid>,
    reason: &str,
) -> Result<(), RuntimeError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
        .bind(app_id)
        .execute(&mut **tx)
        .await
        .map_err(RuntimeError::Schema)?;
    sqlx::query(
        "UPDATE rootcx_system.app_installations
            SET active = FALSE, uninstalled_at = now()
          WHERE app_id = $1 AND active = TRUE",
    )
    .bind(app_id)
    .execute(&mut **tx)
    .await
    .map_err(RuntimeError::Schema)?;
    let grant_ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM rootcx_system.cross_app_collection_grants
          WHERE status IN ('pending', 'active', 'disabled')
            AND (consumer_installation_id IN (
                    SELECT id FROM rootcx_system.app_installations WHERE app_id = $1
                 ) OR provider_installation_id IN (
                    SELECT id FROM rootcx_system.app_installations WHERE app_id = $1
                 ))
          ORDER BY id",
    )
    .bind(app_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(RuntimeError::Schema)?;
    for id in grant_ids {
        lock_grant_tx(tx, id).await.map_err(|error| {
            RuntimeError::Schema(sqlx::Error::Protocol(format!("{error:?}").into()))
        })?;
        let before = load_grant_tx(tx, id).await.map_err(|error| {
            RuntimeError::Schema(sqlx::Error::Protocol(format!("{error:?}").into()))
        })?;
        let changed = sqlx::query(
            "UPDATE rootcx_system.cross_app_collection_grants
                SET status = 'revoked', revoked_at = now(), version = version + 1, updated_at = now()
              WHERE id = $1 AND status IN ('pending', 'active', 'disabled')",
        )
        .bind(id)
        .execute(&mut **tx)
        .await
        .map_err(RuntimeError::Schema)?;
        if changed.rows_affected() == 0 {
            continue;
        }
        let after = load_grant_tx(tx, id).await.map_err(|error| {
            RuntimeError::Schema(sqlx::Error::Protocol(format!("{error:?}").into()))
        })?;
        record_grant_audit_tx(
            tx,
            id,
            "auto_revoked",
            actor_id,
            reason,
            Some(&before),
            &after,
        )
        .await
        .map_err(|error| {
            RuntimeError::Schema(sqlx::Error::Protocol(format!("{error:?}").into()))
        })?;
    }
    Ok(())
}

/// The single authorization Module used by every governed read Adapter.
/// `requested_fields` is the complete set of fields that the query may expose,
/// filter, or sort on.  The Core checks it before opening a provider query.
pub async fn authorize_cross_app_read(
    pool: &PgPool,
    consumer_app: &str,
    provider_app: &str,
    entity: &str,
    action: &str,
    requested_fields: &[String],
) -> Result<AuthorizedRead, ApiError> {
    if !matches!(action, ACTION_LIST | ACTION_READ) {
        return Err(ApiError::Forbidden("cross-app read action is not allowed".into()));
    }
    authorize_cross_app_operation(
        pool, consumer_app, provider_app, entity, action, requested_fields, &[],
    ).await
}

/// Resolve one exact CRUD operation. Query/response fields and supplied payload
/// fields are checked independently; writable scope never grants read access.
pub async fn authorize_cross_app_operation(
    pool: &PgPool,
    consumer_app: &str,
    provider_app: &str,
    entity: &str,
    action: &str,
    requested_fields: &[String],
    written_fields: &[String],
) -> Result<AuthorizedRead, ApiError> {
    expire_stale_grants(pool).await?;
    valid_app_id(consumer_app, "consumer app")?;
    valid_app_id(provider_app, "provider app")?;
    valid_identifier(entity, "entity")?;
    if consumer_app == provider_app {
        return Err(ApiError::Forbidden(
            "cross-app grants cannot target the consumer app".into(),
        ));
    }
    if !valid_action(action) {
        return Err(ApiError::Forbidden(
            "cross-app action is not allowed".into(),
        ));
    }
    let target_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1
               FROM rootcx_system.apps a
              WHERE a.id = $1 AND a.status IN ('installed', 'system')
                AND EXISTS (
                    SELECT 1
                      FROM jsonb_array_elements(
                          COALESCE(a.manifest->'dataContract', '[]'::jsonb)
                      ) entity
                     WHERE entity->>'entityName' = $2
                )
         )",
    )
    .bind(provider_app)
    .bind(entity)
    .fetch_one(pool)
    .await?;
    if !target_exists {
        return Err(stable_not_found());
    }

    let row: Option<(Uuid, Uuid, String, Uuid, i64, i64, i64, Vec<String>, Vec<String>)> = sqlx::query_as(
        "SELECT g.id, g.consumer_installation_id, g.consumer_app,
                g.provider_installation_id, ci.generation, pi.generation,
                g.version, g.field_snapshot, g.write_field_snapshot
           FROM rootcx_system.cross_app_collection_grants g
           JOIN rootcx_system.app_installations ci ON ci.id = g.consumer_installation_id
           JOIN rootcx_system.app_installations pi ON pi.id = g.provider_installation_id
           JOIN rootcx_system.apps ca ON ca.id = g.consumer_app
           JOIN rootcx_system.apps pa ON pa.id = g.provider_app
          WHERE g.consumer_app = $1 AND g.provider_app = $2 AND g.entity = $3
            AND ci.app_id = g.consumer_app AND pi.app_id = g.provider_app
            AND g.status = 'active'
            AND (g.expires_at IS NULL OR g.expires_at > statement_timestamp())
            AND ci.active = TRUE AND pi.active = TRUE
            AND ca.status IN ('installed', 'system')
            AND pa.status IN ('installed', 'system')
            AND $4 = ANY(g.actions)
          ORDER BY g.version DESC LIMIT 1",
    )
    .bind(consumer_app)
    .bind(provider_app)
    .bind(entity)
    .bind(action)
    .fetch_optional(pool)
    .await?;

    let Some((
        grant_id,
        consumer_installation_id,
        grant_consumer,
        provider_installation_id,
        _consumer_generation,
        _provider_generation,
        version,
        field_snapshot,
        write_field_snapshot,
    )) = row
    else {
        return Err(ApiError::Forbidden(
            "cross-app collection access is not authorized".into(),
        ));
    };

    // The query above deliberately joins active installations.  Keep the
    // selected ids in the capability so later adapters can include them in
    // audit events without trusting request data.
    if consumer_installation_id.is_nil() || provider_installation_id.is_nil() {
        return Err(ApiError::Forbidden(
            "cross-app collection access is not authorized".into(),
        ));
    }

    let snapshot: HashSet<&str> = field_snapshot.iter().map(String::as_str).collect();
    for field in requested_fields {
        if !snapshot.contains(field.as_str()) {
            return Err(ApiError::Forbidden(
                "requested collection scope is not authorized".into(),
            ));
        }
    }

    if !matches!(action, ACTION_CREATE | ACTION_UPDATE) && !written_fields.is_empty() {
        return Err(ApiError::Forbidden("this operation cannot write fields".into()));
    }
    for field in written_fields {
        if !write_field_snapshot.contains(field) || crate::manifest::is_system_field(field) {
            return Err(ApiError::Forbidden("requested writable collection scope is not authorized".into()));
        }
    }

    Ok(AuthorizedRead {
        grant_id,
        grant_version: version,
        consumer_app: grant_consumer,
        provider_app: provider_app.to_string(),
        consumer_installation_id,
        provider_installation_id,
        entity: entity.to_string(),
        action: action.to_string(),
        field_snapshot,
        write_field_snapshot,
    })
}

/// Linearize the authorization decision with the provider CRUD operation.
///
/// Authorization first selects an immutable capability snapshot. This function
/// then takes the grant's transaction-scoped Core lock and rechecks every
/// capability field on the *same provider transaction* that will execute the
/// RLS-protected query. Grant transitions use the same advisory key, so a
/// revoke/expiry cannot commit between this recheck and the provider operation.
pub async fn lock_cross_app_read_in_tx(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    authority: &AuthorizedRead,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT pg_advisory_xact_lock(42, hashtext($1))")
        .bind(authority.grant_id.to_string())
        .execute(&mut **tx)
        .await?;

    // This separate statement starts after the lock wait. Its timestamp, unlike
    // now()'s transaction-start timestamp, cannot preserve expired authority.
    let valid: bool = sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1
               FROM rootcx_system.cross_app_collection_grants g
               JOIN rootcx_system.app_installations ci ON ci.id = g.consumer_installation_id
               JOIN rootcx_system.app_installations pi ON pi.id = g.provider_installation_id
               JOIN rootcx_system.apps ca ON ca.id = g.consumer_app
               JOIN rootcx_system.apps pa ON pa.id = g.provider_app
              WHERE g.id = $1
                AND g.consumer_app = $2
                AND g.provider_app = $3
                AND g.entity = $4
                AND g.version = $5
                AND g.consumer_installation_id = $6
                AND g.provider_installation_id = $7
                AND g.status = 'active'
                AND (g.expires_at IS NULL OR g.expires_at > statement_timestamp())
                AND ci.app_id = g.consumer_app
                AND pi.app_id = g.provider_app
                AND ci.active = TRUE
                AND pi.active = TRUE
                AND ca.status IN ('installed', 'system')
                AND pa.status IN ('installed', 'system')
                AND $8 = ANY(g.actions)
                AND g.field_snapshot = $9
                AND g.write_field_snapshot = $10
         )",
    )
    .bind(authority.grant_id)
    .bind(&authority.consumer_app)
    .bind(&authority.provider_app)
    .bind(&authority.entity)
    .bind(authority.grant_version)
    .bind(authority.consumer_installation_id)
    .bind(authority.provider_installation_id)
    .bind(&authority.action)
    .bind(&authority.field_snapshot)
    .bind(&authority.write_field_snapshot)
    .fetch_one(&mut **tx)
    .await?;

    if !valid {
        return Err(sqlx::Error::Protocol(
            "cross-app authorization changed before provider read".into(),
        ));
    }

    Ok(())
}

/// Append-only audit for a governed collection operation. Audit is deliberately a
/// Core-side operation: no application-controlled SQL or payload can forge the
/// source, installation, grant or projection fields.
pub async fn record_read_audit(
    pool: &PgPool,
    authority: Option<&AuthorizedRead>,
    consumer_app: &str,
    provider_app: &str,
    entity: &str,
    action: &str,
    principal_id: Option<Uuid>,
    responsible_human_id: Option<Uuid>,
    outcome: &str,
    denial_category: Option<&str>,
    row_count: Option<i64>,
    correlation_id: Uuid,
) -> Result<(), ApiError> {
    let mut connection = pool.acquire().await?;
    insert_read_audit(
        &mut *connection,
        authority,
        consumer_app,
        provider_app,
        entity,
        action,
        principal_id,
        responsible_human_id,
        outcome,
        denial_category,
        row_count,
        correlation_id,
    )
    .await
    .map_err(|error| ApiError::Internal(format!("cross-app audit unavailable: {error}")))
}

pub(crate) async fn insert_read_audit(
    connection: &mut PgConnection,
    authority: Option<&AuthorizedRead>,
    consumer_app: &str,
    provider_app: &str,
    entity: &str,
    action: &str,
    principal_id: Option<Uuid>,
    responsible_human_id: Option<Uuid>,
    outcome: &str,
    denial_category: Option<&str>,
    row_count: Option<i64>,
    correlation_id: Uuid,
) -> Result<(), sqlx::Error> {
    let (consumer_installation_id, provider_installation_id, grant_id, grant_version, projection) =
        authority.map_or((None, None, None, None, Vec::new()), |a| {
            (
                Some(a.consumer_installation_id),
                Some(a.provider_installation_id),
                Some(a.grant_id),
                Some(a.grant_version),
                a.field_snapshot.clone(),
            )
        });
    sqlx::query(
        "INSERT INTO rootcx_system.cross_app_read_audit
            (consumer_app, provider_app, consumer_installation_id,
             provider_installation_id, entity, action, grant_id, grant_version,
             principal_id, responsible_human_id, outcome, denial_category,
             row_count, correlation_id, projection)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)",
    )
    .bind(consumer_app)
    .bind(provider_app)
    .bind(consumer_installation_id)
    .bind(provider_installation_id)
    .bind(entity)
    .bind(action)
    .bind(grant_id)
    .bind(grant_version)
    .bind(principal_id)
    .bind(responsible_human_id)
    .bind(outcome)
    .bind(denial_category)
    .bind(row_count)
    .bind(correlation_id)
    .bind(projection)
    .execute(&mut *connection)
    .await
    .map(|_| ())
}

/// Project a provider row after RLS has run.  The database never hands the
/// consumer a broader response than the approved snapshot, even if a future
/// provider manifest adds a column before a grant revision is approved.
pub fn project_record(record: JsonValue, field_snapshot: &[String]) -> JsonValue {
    let JsonValue::Object(mut object) = record else {
        return record;
    };
    let allowed: HashSet<&str> = field_snapshot.iter().map(String::as_str).collect();
    object.retain(|key, _| allowed.contains(key.as_str()));
    JsonValue::Object(object)
}

pub fn project_records(records: JsonValue, field_snapshot: &[String]) -> JsonValue {
    match records {
        JsonValue::Array(rows) => JsonValue::Array(
            rows.into_iter()
                .map(|row| project_record(row, field_snapshot))
                .collect(),
        ),
        row => project_record(row, field_snapshot),
    }
}

pub fn project_query_result(result: JsonValue, field_snapshot: &[String]) -> JsonValue {
    let JsonValue::Object(mut object) = result else {
        return project_records(result, field_snapshot);
    };
    if let Some(data) = object.remove("data") {
        object.insert("data".into(), project_records(data, field_snapshot));
        JsonValue::Object(object)
    } else {
        project_record(JsonValue::Object(object), field_snapshot)
    }
}

pub fn query_field_names(where_clause: Option<&JsonValue>, order_by: Option<&str>) -> Vec<String> {
    let mut names = Vec::new();
    if let Some(value) = where_clause {
        collect_filter_fields(value, &mut names);
    }
    if let Some(order_by) = order_by {
        names.push(order_by.to_string());
    }
    names.sort_unstable();
    names.dedup();
    names
}

/// Parse the bounded query envelope shared by the worker IPC and Core tool
/// adapters. Legacy equality maps are handled by the caller; when this helper
/// is used, the envelope's control values are always rejected deterministically
/// instead of silently falling back to defaults.
pub fn parse_query_options(
    object: &serde_json::Map<String, JsonValue>,
) -> Result<(Option<String>, &'static str, i64, i64), String> {
    let order_by = match object.get("orderBy") {
        None => None,
        Some(value) => Some(
            value
                .as_str()
                .ok_or("orderBy must be a string")?
                .to_string(),
        ),
    };
    let direction = match object.get("order") {
        None => "DESC",
        Some(value) => match value.as_str() {
            Some("asc" | "ASC") => "ASC",
            Some("desc" | "DESC") => "DESC",
            _ => return Err("order must be 'asc' or 'desc'".into()),
        },
    };
    let limit = object.get("limit").map_or(Ok(100), |value| {
        value
            .as_i64()
            .ok_or_else(|| "limit must be an integer".to_string())
    })?;
    if !(1..=1000).contains(&limit) {
        return Err("limit must be between 1 and 1000".into());
    }
    let offset = object.get("offset").map_or(Ok(0), |value| {
        value
            .as_i64()
            .ok_or_else(|| "offset must be an integer".to_string())
    })?;
    if offset < 0 {
        return Err("offset must be non-negative".into());
    }
    Ok((order_by, direction, limit, offset))
}

fn collect_filter_fields(value: &JsonValue, output: &mut Vec<String>) {
    let Some(object) = value.as_object() else {
        return;
    };
    for (key, value) in object {
        if !key.starts_with('$') {
            output.push(key.clone());
        } else if matches!(key.as_str(), "$and" | "$or" | "$not") {
            if let Some(items) = value.as_array() {
                for item in items {
                    collect_filter_fields(item, output);
                }
            } else {
                collect_filter_fields(value, output);
            }
        }
    }
}

async fn admin_or_provider_admin(
    pool: &PgPool,
    user_id: Uuid,
    provider_app: &str,
) -> Result<(), ApiError> {
    let global = crate::governance::authority::has_permission_db(
        pool,
        user_id,
        "admin:cross_app.grants.approve",
    )
    .await?;
    let provider = crate::governance::authority::has_permission_db(
        pool,
        user_id,
        &format!("app:{provider_app}:cross_app.approve"),
    )
    .await?;
    if global || provider {
        Ok(())
    } else {
        Err(ApiError::Forbidden(
            "cross-app grant administration denied".into(),
        ))
    }
}

async fn can_view_grants(pool: &PgPool, user_id: Uuid, provider_app: &str) -> Result<(), ApiError> {
    let global_manage =
        crate::governance::authority::has_permission_db(pool, user_id, MANAGEMENT_PERMISSION)
            .await?;
    let global_approve = crate::governance::authority::has_permission_db(
        pool,
        user_id,
        "admin:cross_app.grants.approve",
    )
    .await?;
    let provider = crate::governance::authority::has_permission_db(
        pool,
        user_id,
        &format!("app:{provider_app}:cross_app.approve"),
    )
    .await?;
    if global_manage || global_approve || provider {
        Ok(())
    } else {
        Err(ApiError::Forbidden(
            "cross-app grant administration denied".into(),
        ))
    }
}

async fn expire_stale_grants(pool: &PgPool) -> Result<(), ApiError> {
    let mut tx = pool.begin().await?;
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM rootcx_system.cross_app_collection_grants
          WHERE status IN ('pending', 'active', 'disabled')
            AND expires_at IS NOT NULL AND expires_at <= statement_timestamp()
          ORDER BY id",
    )
    .fetch_all(&mut *tx)
    .await?;
    expire_grants_tx(&mut tx, ids).await?;
    tx.commit().await?;
    Ok(())
}

/// Called after acquiring the creation path's app locks. Only the relationship
/// being replaced is swept, and its expiry history commits with the new grant.
async fn expire_matching_grants_tx(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    consumer_installation: Uuid,
    provider_installation: Uuid,
    entity: &str,
) -> Result<(), ApiError> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM rootcx_system.cross_app_collection_grants
          WHERE consumer_installation_id = $1 AND provider_installation_id = $2
            AND entity = $3 AND status IN ('pending', 'active', 'disabled')
            AND expires_at IS NOT NULL AND expires_at <= statement_timestamp()
          ORDER BY id",
    )
    .bind(consumer_installation)
    .bind(provider_installation)
    .bind(entity)
    .fetch_all(&mut **tx)
    .await?;
    expire_grants_tx(tx, ids).await
}

/// Callers select IDs in ascending order, matching uninstall's grant-lock order.
/// load_grant_tx locks only g; expiry never takes app or installation-row locks.
async fn expire_grants_tx(
    tx: &mut sqlx::Transaction<'_, Postgres>,
    ids: Vec<Uuid>,
) -> Result<(), ApiError> {
    for id in ids {
        lock_grant_tx(tx, id).await?;
        let before = load_grant_tx(tx, id).await?;
        let changed = sqlx::query(
            "UPDATE rootcx_system.cross_app_collection_grants
                SET status = 'expired', version = version + 1, updated_at = now()
              WHERE id = $1 AND status IN ('pending', 'active', 'disabled')
                AND expires_at IS NOT NULL AND expires_at <= statement_timestamp()",
        )
        .bind(id)
        .execute(&mut **tx)
        .await?;
        if changed.rows_affected() == 0 {
            continue;
        }
        let after = load_grant_tx(tx, id).await?;
        record_grant_audit_tx(
            tx,
            id,
            "expired",
            None,
            "grant expired",
            Some(&before),
            &after,
        )
        .await?;
    }
    Ok(())
}

async fn resolve_installation_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    app_id: &str,
) -> Result<(Uuid, i64), ApiError> {
    sqlx::query_as(
        "SELECT id, generation FROM rootcx_system.app_installations
          WHERE app_id = $1 AND active = TRUE
          ORDER BY generation DESC LIMIT 1 FOR UPDATE",
    )
    .bind(app_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(stable_not_found)
}

async fn lock_grant_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    grant_id: Uuid,
) -> Result<(), ApiError> {
    sqlx::query("SELECT pg_advisory_xact_lock(42, hashtext($1))")
        .bind(grant_id.to_string())
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn load_grant(pool: &PgPool, id: Uuid) -> Result<CollectionGrant, ApiError> {
    sqlx::query_as::<_, CollectionGrant>(
        "SELECT g.id, g.consumer_app, g.provider_app,
                g.consumer_installation_id, g.provider_installation_id,
                ci.generation AS consumer_generation,
                pi.generation AS provider_generation,
                g.entity, g.actions,
                g.field_snapshot, g.write_field_snapshot, g.status, g.version, g.expires_at,
                g.requested_by, g.approved_by, g.reason, g.created_at, g.updated_at
           FROM rootcx_system.cross_app_collection_grants g
           JOIN rootcx_system.app_installations ci ON ci.id = g.consumer_installation_id
           JOIN rootcx_system.app_installations pi ON pi.id = g.provider_installation_id
          WHERE g.id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(stable_not_found)
}

async fn load_grant_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: Uuid,
) -> Result<CollectionGrant, ApiError> {
    // Lifecycle changes lock installation rows before waiting for this grant.
    // Lock only g, never the joined installations, to avoid reversing that order.
    sqlx::query_as::<_, CollectionGrant>(
        "SELECT g.id, g.consumer_app, g.provider_app,
                g.consumer_installation_id, g.provider_installation_id,
                ci.generation AS consumer_generation,
                pi.generation AS provider_generation,
                g.entity, g.actions,
                g.field_snapshot, g.write_field_snapshot, g.status, g.version, g.expires_at,
                g.requested_by, g.approved_by, g.reason, g.created_at, g.updated_at
           FROM rootcx_system.cross_app_collection_grants g
           JOIN rootcx_system.app_installations ci ON ci.id = g.consumer_installation_id
           JOIN rootcx_system.app_installations pi ON pi.id = g.provider_installation_id
          WHERE g.id = $1
          FOR UPDATE OF g",
    )
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(stable_not_found)
}

async fn record_grant_audit_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    grant_id: Uuid,
    operation: &str,
    actor_id: Option<Uuid>,
    reason: &str,
    before: Option<&CollectionGrant>,
    after: &CollectionGrant,
) -> Result<(), ApiError> {
    let before_state = before
        .map(serde_json::to_value)
        .transpose()
        .map_err(|error| ApiError::Internal(format!("cannot serialize grant history: {error}")))?;
    let after_state = serde_json::to_value(after)
        .map_err(|error| ApiError::Internal(format!("cannot serialize grant history: {error}")))?;
    sqlx::query(
        "INSERT INTO rootcx_system.cross_app_grant_audit
            (grant_id, operation, actor_id, reason, before_state, after_state)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(grant_id)
    .bind(operation)
    .bind(actor_id)
    .bind(reason)
    .bind(before_state)
    .bind(after_state)
    .execute(&mut **tx)
    .await
    .map_err(|error| ApiError::Internal(format!("cross-app grant audit unavailable: {error}")))?;
    Ok(())
}

/// Do not fill the pool with waiters while an installer needs it for hooks.
/// Acquire both app locks or release the transaction before retrying.
async fn begin_grant_creation(
    pool: &PgPool,
    consumer: &str,
    provider: &str,
) -> Result<sqlx::Transaction<'static, Postgres>, ApiError> {
    let apps = if consumer < provider {
        [consumer, provider]
    } else {
        [provider, consumer]
    };
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let mut tx = pool.begin().await?;
        let mut acquired = true;
        for app in apps {
            let locked: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtext($1))")
                .bind(app)
                .fetch_one(&mut *tx)
                .await?;
            if !locked {
                acquired = false;
                break;
            }
        }
        if acquired {
            return Ok(tx);
        }
        tx.rollback().await?;
        if tokio::time::Instant::now() >= deadline {
            return Err(ApiError::Conflict(
                "application lifecycle is busy; retry grant creation".into(),
            ));
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

fn map_grant_write_error(error: sqlx::Error) -> ApiError {
    if error.as_database_error().is_some_and(|database_error| {
        database_error.code().as_deref() == Some("23505")
            && database_error.constraint() == Some("cross_app_collection_grants_one_active")
    }) {
        ApiError::Conflict("an active or pending grant already exists for this collection".into())
    } else {
        ApiError::Internal(error.to_string())
    }
}

pub async fn create_grant(
    State(rt): State<SharedRuntime>,
    identity: Identity,
    Json(body): Json<CreateGrantRequest>,
) -> Result<(StatusCode, Json<CollectionGrant>), ApiError> {
    let pool = crate::routes::pool(&rt);
    crate::governance::authority::require_perm(&pool, identity.user_id, MANAGEMENT_PERMISSION)
        .await?;
    valid_app_id(&body.consumer_app, "consumer app")?;
    valid_app_id(&body.provider_app, "provider app")?;
    valid_identifier(&body.entity, "entity")?;
    if body.consumer_app == body.provider_app {
        return Err(ApiError::BadRequest(
            "consumer and provider apps must differ".into(),
        ));
    }
    if body
        .expires_at
        .is_some_and(|expires_at| expires_at <= Utc::now())
    {
        return Err(ApiError::BadRequest(
            "expiresAt must be in the future".into(),
        ));
    }
    let actions = validate_actions(&body.actions)?;
    let id = Uuid::new_v4();
    let mut tx = begin_grant_creation(&pool, &body.consumer_app, &body.provider_app).await?;
    let (consumer_id, _) = resolve_installation_tx(&mut tx, &body.consumer_app).await?;
    let (provider_id, _) = resolve_installation_tx(&mut tx, &body.provider_app).await?;
    // Metadata and generation are read only after both lifecycle locks. No
    // pool calls here: holding a transaction must work with a pool of size one.
    let manifest: Option<JsonValue> = sqlx::query_scalar(
        "SELECT manifest FROM rootcx_system.apps
          WHERE id = $1 AND status IN ('installed', 'system')",
    )
    .bind(&body.provider_app)
    .fetch_optional(&mut *tx)
    .await?;
    let manifest: rootcx_types::AppManifest =
        serde_json::from_value(manifest.ok_or_else(stable_not_found)?)
            .map_err(|_| stable_not_found())?;
    let entity = manifest
        .data_contract
        .iter()
        .find(|entity| entity.entity_name == body.entity)
        .ok_or_else(stable_not_found)?;
    let types = crate::data_types::field_types(entity).map_err(ApiError::BadRequest)?;
    let fields = validate_requested_fields(&types, body.fields.as_deref())?;
    let write_fields = validate_write_fields(&types, &actions, body.write_fields.as_deref())?;
    expire_matching_grants_tx(&mut tx, consumer_id, provider_id, &body.entity).await?;
    sqlx::query(
        "INSERT INTO rootcx_system.cross_app_collection_grants
            (id, consumer_app, provider_app, consumer_installation_id,
             provider_installation_id, entity, actions, field_snapshot,
             status, requested_by, expires_at, reason, write_field_snapshot)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'pending', $9, $10, $11, $12)",
    )
    .bind(id)
    .bind(&body.consumer_app)
    .bind(&body.provider_app)
    .bind(consumer_id)
    .bind(provider_id)
    .bind(&body.entity)
    .bind(&actions)
    .bind(&fields)
    .bind(identity.user_id)
    .bind(body.expires_at)
    .bind(&body.reason)
    .bind(&write_fields)
    .execute(&mut *tx)
    .await
    .map_err(map_grant_write_error)?;

    let grant = load_grant_tx(&mut tx, id).await?;
    record_grant_audit_tx(
        &mut tx,
        id,
        "created",
        Some(identity.user_id),
        &body.reason,
        None,
        &grant,
    )
    .await?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(grant)))
}

pub async fn list_grants(
    State(rt): State<SharedRuntime>,
    identity: Identity,
    Query(query): Query<GrantListQuery>,
) -> Result<Json<Vec<CollectionGrant>>, ApiError> {
    let pool = crate::routes::pool(&rt);
    let global = crate::governance::authority::has_permission_db(
        &pool,
        identity.user_id,
        MANAGEMENT_PERMISSION,
    )
    .await?
        || crate::governance::authority::has_permission_db(
            &pool,
            identity.user_id,
            "admin:cross_app.grants.approve",
        )
        .await?;
    if !global {
        let provider = query.provider_app.as_deref().ok_or_else(|| {
            ApiError::Forbidden(
                "providerApp is required for provider-scoped grant administration".into(),
            )
        })?;
        valid_app_id(provider, "provider app")?;
        can_view_grants(&pool, identity.user_id, provider).await?;
    }
    if let Some(status) = query.status.as_deref() {
        validate_status(status)?;
    }
    expire_stale_grants(&pool).await?;
    let rows = sqlx::query_as::<_, CollectionGrant>(
        "SELECT g.id, g.consumer_app, g.provider_app,
                g.consumer_installation_id, g.provider_installation_id,
                ci.generation AS consumer_generation,
                pi.generation AS provider_generation, g.entity, g.actions,
                g.field_snapshot, g.write_field_snapshot, g.status, g.version, g.expires_at,
                g.requested_by, g.approved_by, g.reason, g.created_at, g.updated_at
           FROM rootcx_system.cross_app_collection_grants g
           JOIN rootcx_system.app_installations ci ON ci.id = g.consumer_installation_id
           JOIN rootcx_system.app_installations pi ON pi.id = g.provider_installation_id
          WHERE ($1 OR g.provider_app = $2)
            AND ($3::text IS NULL OR g.status = $3)
          ORDER BY g.created_at DESC",
    )
    .bind(global)
    .bind(query.provider_app.as_deref())
    .bind(query.status.as_deref())
    .fetch_all(&pool)
    .await?;
    Ok(Json(rows))
}

pub async fn get_grant(
    State(rt): State<SharedRuntime>,
    identity: Identity,
    Path(id): Path<Uuid>,
) -> Result<Json<CollectionGrant>, ApiError> {
    let pool = crate::routes::pool(&rt);
    expire_stale_grants(&pool).await?;
    let grant = load_grant(&pool, id).await?;
    can_view_grants(&pool, identity.user_id, &grant.provider_app).await?;
    Ok(Json(grant))
}

pub async fn approve_grant(
    State(rt): State<SharedRuntime>,
    identity: Identity,
    Path(id): Path<Uuid>,
    Json(reason): Json<GrantReason>,
) -> Result<Json<CollectionGrant>, ApiError> {
    let pool = crate::routes::pool(&rt);
    expire_stale_grants(&pool).await?;
    let grant = load_grant(&pool, id).await?;
    admin_or_provider_admin(&pool, identity.user_id, &grant.provider_app).await?;
    let mut tx = pool.begin().await?;
    lock_grant_tx(&mut tx, id).await?;
    let before = load_grant_tx(&mut tx, id).await?;
    let result = sqlx::query(
        "UPDATE rootcx_system.cross_app_collection_grants
            SET status = 'active', approved_by = $2, reason = CASE WHEN $3 = '' THEN reason ELSE $3 END,
                version = version + 1, updated_at = now()
          WHERE id = $1 AND status = 'pending'
            AND (expires_at IS NULL OR expires_at > statement_timestamp())",
    )
    .bind(id)
    .bind(identity.user_id)
    .bind(&reason.reason)
    .execute(&mut *tx)
    .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::Conflict("grant is no longer pending".into()));
    }
    let after = load_grant_tx(&mut tx, id).await?;
    record_grant_audit_tx(
        &mut tx,
        id,
        "approved",
        Some(identity.user_id),
        &reason.reason,
        Some(&before),
        &after,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(after))
}

pub async fn disable_grant(
    State(rt): State<SharedRuntime>,
    identity: Identity,
    Path(id): Path<Uuid>,
    Json(reason): Json<GrantReason>,
) -> Result<Json<CollectionGrant>, ApiError> {
    let pool = crate::routes::pool(&rt);
    crate::governance::authority::require_perm(&pool, identity.user_id, MANAGEMENT_PERMISSION)
        .await?;
    let mut tx = pool.begin().await?;
    lock_grant_tx(&mut tx, id).await?;
    let before = load_grant_tx(&mut tx, id).await?;
    let result = sqlx::query(
        "UPDATE rootcx_system.cross_app_collection_grants
            SET status = CASE WHEN status = 'active' THEN 'disabled' ELSE status END,
                reason = CASE WHEN $2 = '' THEN reason ELSE $2 END,
                version = version + 1, updated_at = now()
          WHERE id = $1 AND status IN ('active', 'disabled')",
    )
    .bind(id)
    .bind(&reason.reason)
    .execute(&mut *tx)
    .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::Conflict("grant cannot be disabled".into()));
    }
    let after = load_grant_tx(&mut tx, id).await?;
    record_grant_audit_tx(
        &mut tx,
        id,
        "disabled",
        Some(identity.user_id),
        &reason.reason,
        Some(&before),
        &after,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(after))
}

pub async fn enable_grant(
    State(rt): State<SharedRuntime>,
    identity: Identity,
    Path(id): Path<Uuid>,
    Json(reason): Json<GrantReason>,
) -> Result<Json<CollectionGrant>, ApiError> {
    let pool = crate::routes::pool(&rt);
    expire_stale_grants(&pool).await?;
    let grant = load_grant(&pool, id).await?;
    admin_or_provider_admin(&pool, identity.user_id, &grant.provider_app).await?;
    let mut tx = pool.begin().await?;
    lock_grant_tx(&mut tx, id).await?;
    let before = load_grant_tx(&mut tx, id).await?;
    let result = sqlx::query(
        "UPDATE rootcx_system.cross_app_collection_grants
            SET status = 'active',
                reason = CASE WHEN $2 = '' THEN reason ELSE $2 END,
                version = version + 1, updated_at = now()
          WHERE id = $1 AND status = 'disabled'
            AND (expires_at IS NULL OR expires_at > statement_timestamp())",
    )
    .bind(id)
    .bind(&reason.reason)
    .execute(&mut *tx)
    .await
    .map_err(map_grant_write_error)?;
    if result.rows_affected() == 0 {
        return Err(ApiError::Conflict(
            "grant cannot be enabled (it may be expired or terminal)".into(),
        ));
    }
    let after = load_grant_tx(&mut tx, id).await?;
    record_grant_audit_tx(
        &mut tx,
        id,
        "enabled",
        Some(identity.user_id),
        &reason.reason,
        Some(&before),
        &after,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(after))
}

pub async fn revoke_grant(
    State(rt): State<SharedRuntime>,
    identity: Identity,
    Path(id): Path<Uuid>,
    Json(reason): Json<GrantReason>,
) -> Result<Json<CollectionGrant>, ApiError> {
    let pool = crate::routes::pool(&rt);
    crate::governance::authority::require_perm(&pool, identity.user_id, MANAGEMENT_PERMISSION)
        .await?;
    let mut tx = pool.begin().await?;
    lock_grant_tx(&mut tx, id).await?;
    let before = load_grant_tx(&mut tx, id).await?;
    let result = sqlx::query(
        "UPDATE rootcx_system.cross_app_collection_grants
            SET status = 'revoked', revoked_at = now(),
                reason = CASE WHEN $2 = '' THEN reason ELSE $2 END,
                version = version + 1, updated_at = now()
          WHERE id = $1 AND status NOT IN ('revoked', 'expired')",
    )
    .bind(id)
    .bind(&reason.reason)
    .execute(&mut *tx)
    .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::Conflict("grant is already terminal".into()));
    }
    let after = load_grant_tx(&mut tx, id).await?;
    record_grant_audit_tx(
        &mut tx,
        id,
        "revoked",
        Some(identity.user_id),
        &reason.reason,
        Some(&before),
        &after,
    )
    .await?;
    tx.commit().await?;
    Ok(Json(after))
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct CrossAppReadAudit {
    pub id: Uuid,
    pub consumer_app: String,
    pub provider_app: String,
    pub consumer_installation_id: Option<Uuid>,
    pub provider_installation_id: Option<Uuid>,
    pub entity: String,
    pub action: String,
    pub grant_id: Option<Uuid>,
    pub grant_version: Option<i64>,
    pub principal_id: Option<Uuid>,
    pub responsible_human_id: Option<Uuid>,
    pub outcome: String,
    pub denial_category: Option<String>,
    pub row_count: Option<i64>,
    pub correlation_id: Uuid,
    pub projection: Vec<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AuditListQuery {
    pub grant_id: Option<Uuid>,
    #[serde(default = "default_audit_limit")]
    pub limit: i64,
}

fn default_audit_limit() -> i64 {
    100
}

pub async fn list_read_audit(
    State(rt): State<SharedRuntime>,
    identity: Identity,
    Query(query): Query<AuditListQuery>,
) -> Result<Json<Vec<CrossAppReadAudit>>, ApiError> {
    let pool = crate::routes::pool(&rt);
    crate::governance::authority::require_perm(&pool, identity.user_id, MANAGEMENT_PERMISSION)
        .await?;
    let limit = query.limit.clamp(1, 1000);
    let rows = if let Some(grant_id) = query.grant_id {
        sqlx::query_as::<_, CrossAppReadAudit>(
            "SELECT id, consumer_app, provider_app, consumer_installation_id,
                    provider_installation_id, entity, action, grant_id,
                    grant_version, principal_id, responsible_human_id, outcome,
                    denial_category, row_count, correlation_id, projection, created_at
               FROM rootcx_system.cross_app_read_audit
              WHERE grant_id = $1 ORDER BY created_at DESC LIMIT $2",
        )
        .bind(grant_id)
        .bind(limit)
        .fetch_all(&pool)
        .await?
    } else {
        sqlx::query_as::<_, CrossAppReadAudit>(
            "SELECT id, consumer_app, provider_app, consumer_installation_id,
                    provider_installation_id, entity, action, grant_id,
                    grant_version, principal_id, responsible_human_id, outcome,
                    denial_category, row_count, correlation_id, projection, created_at
               FROM rootcx_system.cross_app_read_audit
              ORDER BY created_at DESC LIMIT $1",
        )
        .bind(limit)
        .fetch_all(&pool)
        .await?
    };
    Ok(Json(rows))
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GrantHistoryQuery {
    #[serde(default = "default_audit_limit")]
    pub limit: i64,
}

pub async fn list_grant_audit(
    State(rt): State<SharedRuntime>,
    identity: Identity,
    Path(grant_id): Path<Uuid>,
    Query(query): Query<GrantHistoryQuery>,
) -> Result<Json<Vec<CrossAppGrantAudit>>, ApiError> {
    let pool = crate::routes::pool(&rt);
    let grant = load_grant(&pool, grant_id).await?;
    can_view_grants(&pool, identity.user_id, &grant.provider_app).await?;
    let limit = query.limit.clamp(1, 1000);
    let rows = sqlx::query_as::<_, CrossAppGrantAudit>(
        "SELECT id, grant_id, operation, actor_id, reason, before_state,
                after_state, created_at
           FROM rootcx_system.cross_app_grant_audit
          WHERE grant_id = $1 ORDER BY created_at DESC LIMIT $2",
    )
    .bind(grant_id)
    .bind(limit)
    .fetch_all(&pool)
    .await?;
    Ok(Json(rows))
}

pub fn routes() -> Router<SharedRuntime> {
    Router::new()
        .route(
            "/api/v1/cross-app/grants",
            get(list_grants).post(create_grant),
        )
        .route("/api/v1/cross-app/grants/{id}", get(get_grant))
        .route("/api/v1/cross-app/grants/{id}/approve", post(approve_grant))
        .route("/api/v1/cross-app/grants/{id}/disable", post(disable_grant))
        .route("/api/v1/cross-app/grants/{id}/enable", post(enable_grant))
        .route("/api/v1/cross-app/grants/{id}/revoke", post(revoke_grant))
        .route("/api/v1/cross-app/grants/{id}/audit", get(list_grant_audit))
        .route("/api/v1/cross-app/audit", get(list_read_audit))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn actions_are_crud_only_and_deduplicated() {
        let actions = validate_actions(&["read".into(), "list".into(), "read".into()]).unwrap();
        assert_eq!(actions, vec!["list", "read"]);
        assert_eq!(validate_actions(&["delete".into(), "update".into(), "create".into()]).unwrap(),
            vec!["create", "delete", "update"]);
        for actions in [vec![], vec!["execute".into()], vec!["read".into(), "*".into()]] {
            assert!(validate_actions(&actions).is_err(), "{actions:?}");
        }
    }

    #[test]
    fn grant_status_filters_are_closed_to_the_lifecycle() {
        for status in ["pending", "active", "disabled", "revoked", "expired"] {
            assert!(
                validate_status(status).is_ok(),
                "status should be accepted: {status}"
            );
        }
        assert!(validate_status("anything").is_err());
    }

    #[test]
    fn query_options_are_strict_and_bounded() {
        let object = serde_json::json!({
            "orderBy": "name",
            "order": "asc",
            "limit": 25,
            "offset": 10,
        })
        .as_object()
        .cloned()
        .unwrap();
        assert_eq!(
            parse_query_options(&object).unwrap(),
            (Some("name".into()), "ASC", 25, 10)
        );

        for (key, value, error) in [
            ("limit", serde_json::json!("25"), "limit must be an integer"),
            ("limit", json!(0), "limit must be between 1 and 1000"),
            ("limit", json!(1001), "limit must be between 1 and 1000"),
            ("limit", json!(1.5), "limit must be an integer"),
            ("offset", serde_json::json!(-1), "offset must be non-negative"),
            ("offset", json!("0"), "offset must be an integer"),
            ("orderBy", json!(false), "orderBy must be a string"),
            ("order", serde_json::json!("sideways"), "order must be 'asc' or 'desc'"),
        ] {
            let object = serde_json::json!({key: value}).as_object().cloned().unwrap();
            assert_eq!(parse_query_options(&object).unwrap_err(), error, "{object:?}");
        }
        for limit in [1, 1000] {
            let object = json!({"limit": limit, "offset": 0}).as_object().unwrap().clone();
            assert_eq!(parse_query_options(&object).unwrap().2, limit, "{object:?}");
        }
    }

    #[test]
    fn field_snapshot_is_explicit_and_always_keeps_safe_system_fields() {
        let readable = |field_type| crate::data_types::Field {
            field_type,
            sensitive: false,
        };
        let types = [
            ("id".into(), readable(crate::data_types::FieldType::Uuid)),
            (
                "created_at".into(),
                readable(crate::data_types::FieldType::Timestamp),
            ),
            (
                "updated_at".into(),
                readable(crate::data_types::FieldType::Timestamp),
            ),
            ("name".into(), readable(crate::data_types::FieldType::Text)),
            (
                "secret".into(),
                crate::data_types::Field {
                    field_type: crate::data_types::FieldType::Text,
                    sensitive: true,
                },
            ),
        ]
        .into_iter()
        .collect();
        let fields = validate_requested_fields(&types, None).unwrap();
        assert!(fields.iter().any(|field| field == "name"));
        assert!(fields.iter().any(|field| field == "id"));
        assert!(!fields.iter().any(|field| field == "secret"));
        for fields in [vec![], vec!["secret".into()], vec!["unknown".into()]] {
            assert!(validate_requested_fields(&types, Some(&fields)).is_err(), "{fields:?}");
        }
        assert_eq!(
            validate_requested_fields(&types, Some(&["name".into(), "name".into()])).unwrap(),
            vec!["created_at", "id", "name", "updated_at"],
        );
    }

    #[test]
    fn write_scope_requires_explicit_safe_fields_for_each_mutating_action() {
        let types = ["id", "created_at", "updated_at", "name", "secret"]
            .into_iter()
            .map(|name| (name.into(), crate::data_types::Field {
                field_type: crate::data_types::FieldType::Text,
                sensitive: name == "secret",
            }))
            .collect();
        for action in ["create", "update"] {
            let actions = vec![action.into()];
            for fields in [
                None, Some(vec![]), Some(vec!["unknown".into()]),
                Some(vec!["secret".into()]), Some(vec!["id".into()]),
                Some(vec!["created_at".into()]), Some(vec!["updated_at".into()]),
                Some(vec!["name".into(), "secret".into()]),
            ] {
                assert!(
                    validate_write_fields(&types, &actions, fields.as_deref()).is_err(),
                    "{action} with {fields:?}",
                );
            }
            assert_eq!(
                validate_write_fields(&types, &actions, Some(&["name".into(), "name".into()])).unwrap(),
                vec!["name"], "{action}",
            );
        }
        for action in ["list", "read", "delete"] {
            let actions = vec![action.into()];
            assert!(validate_write_fields(&types, &actions, None).unwrap().is_empty(), "{action}");
            assert!(
                validate_write_fields(&types, &actions, Some(&["name".into()])).is_err(),
                "{action} must not accept a write projection",
            );
        }
    }

    #[test]
    fn projection_cannot_reveal_a_new_or_unapproved_field() {
        let row = json!({"id": "1", "name": "Ada", "future": "hidden"});
        assert_eq!(
            project_record(row.clone(), &["id".into(), "name".into()]),
            json!({"id": "1", "name": "Ada"})
        );
        assert_eq!(
            project_query_result(
                json!({"id": "1", "name": "Ada", "status": "private"}),
                &["id".into(), "name".into()],
            ),
            json!({"id": "1", "name": "Ada"}),
            "findOne must use the same frozen projection as list results",
        );
        assert_eq!(
            project_query_result(
                json!({"data": [row], "total": 1}),
                &["id".into(), "name".into()],
            ),
            json!({"data": [{"id": "1", "name": "Ada"}], "total": 1}),
        );
    }

    #[test]
    fn query_field_collection_finds_nested_filter_and_order_keys() {
        let query = json!({"$or": [{"status": {"$eq": "open"}}, {"owner": "ada"}]});
        assert_eq!(
            query_field_names(Some(&query), Some("created_at")),
            vec!["created_at", "owner", "status"]
        );
    }

    #[test]
    fn query_field_collection_does_not_treat_json_operands_as_columns() {
        let query = json!({"metadata": {"$eq": {"nested": "value"}}});
        assert_eq!(query_field_names(Some(&query), None), vec!["metadata"]);
    }

    #[test]
    fn identifiers_fail_closed_before_sql_can_be_built() {
        for value in ["", "123app", "App", "app;drop", "app id"] {
            assert!(valid_identifier(value, "app").is_err(), "{value:?}");
        }
    }
}
