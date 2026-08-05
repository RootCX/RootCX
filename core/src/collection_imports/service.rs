use std::collections::HashSet;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde_json::{Value as JsonValue, json};
use sqlx::PgPool;
use uuid::Uuid;

use super::{
    CollectionImport, CreateImportRequest, ImportMode, STATUS_CANCELLED, STATUS_FAILED,
    STATUS_PENDING,
};
use crate::api_error::ApiError;
use crate::auth::{identity::Identity, secure_tokens};
use crate::governance::authority::{has_permission, intersect_permissions, resolve_permissions};
use crate::governance::enforcement::ContextState;
use crate::manifest::{entity_exists, field_type_map, is_system_field};
use crate::routes::SharedRuntime;

const IMPORT_SELECT: &str = r#"SELECT id, app_id, entity, mode, columns, conflict_columns, allow_empty,
    source_file_id, source_checksum, idempotency_key, status, rows_loaded,
    bytes_received, cancel_requested, error, created_at::text, started_at::text,
    completed_at::text, NULL::text AS upload_url
    FROM rootcx_system.collection_imports"#;

fn is_unique_violation(error: &sqlx::Error) -> bool {
    error.as_database_error().and_then(|e| e.code()).as_deref() == Some("23505")
}

fn direct_state(identity: &Identity) -> ContextState {
    ContextState {
        user_id: Some(identity.user_id),
        is_delegated: false,
        effective_perms: vec![],
        connection_id: None,
    }
}

async fn effective_permissions(
    pool: &PgPool,
    state: &ContextState,
) -> Result<Vec<String>, ApiError> {
    let user_id = state.user_id.ok_or_else(|| {
        ApiError::Forbidden("collection imports require an authenticated user".into())
    })?;
    if !crate::auth::identity::principal_enabled(pool, user_id).await {
        return Err(ApiError::Forbidden("principal is disabled".into()));
    }
    let (_, current) = resolve_permissions(pool, user_id).await?;
    Ok(if state.is_delegated {
        intersect_permissions(&current, &state.effective_perms)
    } else {
        current
    })
}

fn require_permissions(
    permissions: &[String],
    app_id: &str,
    entity: &str,
    mode: ImportMode,
) -> Result<(), ApiError> {
    for action in mode.required_actions() {
        let required = format!("app:{app_id}:{entity}.{action}");
        if !has_permission(permissions, &required) {
            return Err(ApiError::Forbidden(format!(
                "permission denied: {required}"
            )));
        }
    }
    Ok(())
}

async fn validate_request(
    pool: &PgPool,
    app_id: &str,
    entity: &str,
    request: &CreateImportRequest,
) -> Result<(), ApiError> {
    crate::routes::crud::validate_app_id(app_id)?;
    if !entity_exists(pool, app_id, entity).await? {
        return Err(ApiError::NotFound(format!(
            "entity '{entity}' not found in app '{app_id}'"
        )));
    }
    if request.columns.is_empty() || request.columns.len() > 256 {
        return Err(ApiError::BadRequest(
            "columns must contain between 1 and 256 fields".into(),
        ));
    }
    let mut seen = HashSet::new();
    let fields = field_type_map(pool, app_id, entity).await?;
    for column in &request.columns {
        if is_system_field(column) || !fields.contains_key(column) {
            return Err(ApiError::BadRequest(format!(
                "unknown or read-only column: {column}"
            )));
        }
        if !seen.insert(column) {
            return Err(ApiError::BadRequest(format!("duplicate column: {column}")));
        }
    }
    match request.mode {
        ImportMode::Upsert if request.conflict_columns.is_empty() => {
            return Err(ApiError::BadRequest(
                "upsert requires conflictColumns".into(),
            ));
        }
        ImportMode::Append | ImportMode::Replace if !request.conflict_columns.is_empty() => {
            return Err(ApiError::BadRequest(
                "conflictColumns are only valid for upsert".into(),
            ));
        }
        _ => {}
    }
    if request
        .conflict_columns
        .iter()
        .any(|column| !seen.contains(column))
    {
        return Err(ApiError::BadRequest(
            "conflictColumns must be included in columns".into(),
        ));
    }
    let unique_conflicts = request.conflict_columns.iter().collect::<HashSet<_>>();
    if unique_conflicts.len() != request.conflict_columns.len() {
        return Err(ApiError::BadRequest(
            "conflictColumns must not contain duplicates".into(),
        ));
    }
    if matches!(request.mode, ImportMode::Upsert) {
        let matches_unique_index: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                SELECT 1
                FROM pg_catalog.pg_index idx
                JOIN pg_catalog.pg_class table_class ON table_class.oid = idx.indrelid
                JOIN pg_catalog.pg_namespace namespace ON namespace.oid = table_class.relnamespace
                WHERE namespace.nspname = $1
                  AND table_class.relname = $2
                  AND idx.indisunique
                  AND idx.indisvalid
                  AND idx.indpred IS NULL
                  AND (
                    SELECT array_agg(attribute.attname::text ORDER BY attribute.attname::text)
                    FROM unnest(idx.indkey::smallint[]) WITH ORDINALITY AS key(attnum, position)
                    JOIN pg_catalog.pg_attribute attribute
                      ON attribute.attrelid = table_class.oid AND attribute.attnum = key.attnum
                    WHERE key.position <= idx.indnkeyatts
                  ) = (
                    SELECT array_agg(column_name ORDER BY column_name)
                    FROM unnest($3::text[]) AS column_name
                  )
            )",
        )
        .bind(app_id)
        .bind(entity)
        .bind(&request.conflict_columns)
        .fetch_one(pool)
        .await?;
        if !matches_unique_index {
            return Err(ApiError::BadRequest(
                "conflictColumns must exactly match a non-partial unique index".into(),
            ));
        }
    }
    if request
        .idempotency_key
        .as_ref()
        .is_some_and(|key| key.is_empty() || key.len() > 200)
    {
        return Err(ApiError::BadRequest(
            "idempotencyKey must contain 1 to 200 characters".into(),
        ));
    }
    Ok(())
}

async fn source_checksum(
    pool: &PgPool,
    app_id: &str,
    file_id: Option<Uuid>,
    permissions: &[String],
) -> Result<Option<String>, ApiError> {
    let Some(file_id) = file_id else {
        return Ok(None);
    };
    let required = format!("app:{app_id}:storage.read");
    if !has_permission(permissions, &required) {
        return Err(ApiError::Forbidden(format!(
            "permission denied: {required}"
        )));
    }
    let checksum = sqlx::query_as::<_, (Option<String>,)>(
        "SELECT checksum FROM rootcx_system.files WHERE id = $1 AND app_id = $2",
    )
    .bind(file_id)
    .bind(app_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("file {file_id} not found")))?;
    Ok(checksum.0)
}

fn upload_url(runtime_url: &str, token: &str) -> String {
    format!("{runtime_url}/api/v1/collection-import-uploads/{token}")
}

async fn issue_token(
    pool: &PgPool,
    import_id: Uuid,
    runtime_url: &str,
) -> Result<String, ApiError> {
    let token = secure_tokens::generate();
    let result = sqlx::query(
        "UPDATE rootcx_system.collection_imports
         SET token_hash = $2, token_expires_at = now() + interval '1 hour', updated_at = now()
         WHERE id = $1 AND status = 'pending'",
    )
    .bind(import_id)
    .bind(secure_tokens::hash(&token).as_slice())
    .execute(pool)
    .await?;
    if result.rows_affected() != 1 {
        return Err(ApiError::Conflict(
            "collection import is no longer pending".into(),
        ));
    }
    Ok(upload_url(runtime_url, &token))
}

async fn find_import(
    pool: &PgPool,
    app_id: &str,
    entity: &str,
    id: Uuid,
) -> Result<CollectionImport, ApiError> {
    sqlx::query_as::<_, CollectionImport>(&format!(
        "{IMPORT_SELECT} WHERE app_id = $1 AND entity = $2 AND id = $3"
    ))
    .bind(app_id)
    .bind(entity)
    .bind(id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::NotFound(format!("collection import {id} not found")))
}

async fn reuse_idempotent_import(
    pool: &PgPool,
    runtime_url: &str,
    app_id: &str,
    entity: &str,
    actor_uid: Uuid,
    request: &CreateImportRequest,
    checksum: &Option<String>,
) -> Result<Option<CollectionImport>, ApiError> {
    let Some(key) = &request.idempotency_key else {
        return Ok(None);
    };
    let existing = sqlx::query_as::<_, CollectionImport>(&format!(
        "{IMPORT_SELECT} WHERE app_id = $1 AND entity = $2 AND actor_uid = $3 AND idempotency_key = $4"
    ))
    .bind(app_id)
    .bind(entity)
    .bind(actor_uid)
    .bind(key)
    .fetch_optional(pool)
    .await?;
    let Some(mut existing) = existing else {
        return Ok(None);
    };
    let same_request = existing.mode == request.mode.as_str()
        && existing.columns == request.columns
        && existing.conflict_columns == request.conflict_columns
        && existing.allow_empty == request.allow_empty
        && existing.source_file_id == request.source_file_id
        && existing.source_checksum.as_ref() == checksum.as_ref();
    if !same_request {
        return Err(ApiError::Conflict(
            "idempotencyKey is already associated with a different import request".into(),
        ));
    }
    if existing.status == STATUS_PENDING {
        existing.upload_url = Some(issue_token(pool, existing.id, runtime_url).await?);
    }
    Ok(Some(existing))
}

async fn create(
    pool: &PgPool,
    runtime_url: &str,
    app_id: &str,
    entity: &str,
    request: CreateImportRequest,
    state: ContextState,
) -> Result<CollectionImport, ApiError> {
    validate_request(pool, app_id, entity, &request).await?;
    let permissions = effective_permissions(pool, &state).await?;
    require_permissions(&permissions, app_id, entity, request.mode)?;
    let actor_uid = state
        .user_id
        .expect("effective_permissions requires user id");
    let checksum = source_checksum(pool, app_id, request.source_file_id, &permissions).await?;

    if let Some(existing) = reuse_idempotent_import(
        pool,
        runtime_url,
        app_id,
        entity,
        actor_uid,
        &request,
        &checksum,
    )
    .await?
    {
        return Ok(existing);
    }

    let active: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM rootcx_system.collection_imports
         WHERE app_id = $1 AND entity = $2 AND status IN ('pending', 'loading', 'publishing')",
    )
    .bind(app_id)
    .bind(entity)
    .fetch_optional(pool)
    .await?;
    if let Some(id) = active {
        return Err(ApiError::Conflict(format!(
            "collection import {id} is already active"
        )));
    }

    let token = secure_tokens::generate();
    let id = Uuid::new_v4();
    let insert = sqlx::query(
        "INSERT INTO rootcx_system.collection_imports (
            id, app_id, entity, mode, columns, conflict_columns, allow_empty, source_file_id,
            source_checksum, idempotency_key, actor_uid, is_delegated,
            effective_perms, token_hash, token_expires_at
         ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,now() + interval '1 hour')",
    )
    .bind(id)
    .bind(app_id)
    .bind(entity)
    .bind(request.mode.as_str())
    .bind(&request.columns)
    .bind(&request.conflict_columns)
    .bind(request.allow_empty)
    .bind(request.source_file_id)
    .bind(&checksum)
    .bind(&request.idempotency_key)
    .bind(actor_uid)
    .bind(state.is_delegated)
    .bind(&permissions)
    .bind(secure_tokens::hash(&token).as_slice())
    .execute(pool)
    .await;
    if let Err(error) = insert {
        if is_unique_violation(&error) {
            if let Some(existing) = reuse_idempotent_import(
                pool,
                runtime_url,
                app_id,
                entity,
                actor_uid,
                &request,
                &checksum,
            )
            .await?
            {
                return Ok(existing);
            }
            return Err(ApiError::Conflict(
                "another import became active for this collection".into(),
            ));
        }
        return Err(error.into());
    }

    let mut created = find_import(pool, app_id, entity, id).await?;
    created.upload_url = Some(upload_url(runtime_url, &token));
    Ok(created)
}

pub(crate) async fn create_for_worker(
    pool: &PgPool,
    runtime_url: &str,
    app_id: &str,
    entity: &str,
    params: JsonValue,
    state: ContextState,
) -> Result<JsonValue, String> {
    let request: CreateImportRequest = serde_json::from_value(params).map_err(|e| e.to_string())?;
    create(pool, runtime_url, app_id, entity, request, state)
        .await
        .and_then(|value| {
            serde_json::to_value(value).map_err(|e| ApiError::Internal(e.to_string()))
        })
        .map_err(|error| match error {
            ApiError::NotFound(message)
            | ApiError::BadRequest(message)
            | ApiError::Unauthorized(message)
            | ApiError::Forbidden(message)
            | ApiError::Conflict(message)
            | ApiError::Internal(message) => message,
            ApiError::NotReady => "runtime not ready".into(),
        })
}

pub(crate) async fn create_http(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((app_id, entity)): Path<(String, String)>,
    Json(request): Json<CreateImportRequest>,
) -> Result<(StatusCode, Json<CollectionImport>), ApiError> {
    let created = create(
        rt.pool(),
        rt.runtime_url(),
        &app_id,
        &entity,
        request,
        direct_state(&identity),
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(created)))
}

pub(crate) async fn get_http(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((app_id, entity, id)): Path<(String, String, Uuid)>,
) -> Result<Json<CollectionImport>, ApiError> {
    let import = find_import(rt.pool(), &app_id, &entity, id).await?;
    authorize_read(rt.pool(), &identity, &import).await?;
    Ok(Json(import))
}

pub(crate) async fn list_http(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((app_id, entity)): Path<(String, String)>,
) -> Result<Json<Vec<CollectionImport>>, ApiError> {
    let permissions = resolve_permissions(rt.pool(), identity.user_id).await?.1;
    let can_read_collection = has_permission(&permissions, &format!("app:{app_id}:{entity}.read"));
    let sql = if can_read_collection {
        format!(
            "{IMPORT_SELECT} WHERE app_id = $1 AND entity = $2 ORDER BY created_at DESC LIMIT 100"
        )
    } else {
        format!(
            "{IMPORT_SELECT} WHERE app_id = $1 AND entity = $2 AND actor_uid = $3 ORDER BY created_at DESC LIMIT 100"
        )
    };
    let mut query = sqlx::query_as::<_, CollectionImport>(&sql)
        .bind(app_id)
        .bind(entity);
    if !can_read_collection {
        query = query.bind(identity.user_id);
    }
    let imports = query.fetch_all(rt.pool()).await?;
    Ok(Json(imports))
}

async fn authorize_read(
    pool: &PgPool,
    identity: &Identity,
    import: &CollectionImport,
) -> Result<(), ApiError> {
    let actor: Uuid =
        sqlx::query_scalar("SELECT actor_uid FROM rootcx_system.collection_imports WHERE id = $1")
            .bind(import.id)
            .fetch_one(pool)
            .await?;
    if actor == identity.user_id {
        return Ok(());
    }
    crate::governance::authority::require_perm(
        pool,
        identity.user_id,
        &format!("app:{}:{}.read", import.app_id, import.entity),
    )
    .await
}

async fn authorize_control(
    pool: &PgPool,
    identity: &Identity,
    import: &CollectionImport,
) -> Result<Vec<String>, ApiError> {
    let actor: Uuid =
        sqlx::query_scalar("SELECT actor_uid FROM rootcx_system.collection_imports WHERE id = $1")
            .bind(import.id)
            .fetch_one(pool)
            .await?;
    let permissions = resolve_permissions(pool, identity.user_id).await?.1;
    if actor != identity.user_id {
        let import_mode = ImportMode::parse(&import.mode)?;
        require_permissions(&permissions, &import.app_id, &import.entity, import_mode)?;
    }
    Ok(permissions)
}

pub(crate) async fn cancel_http(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((app_id, entity, id)): Path<(String, String, Uuid)>,
) -> Result<Json<JsonValue>, ApiError> {
    let import = find_import(rt.pool(), &app_id, &entity, id).await?;
    authorize_control(rt.pool(), &identity, &import).await?;
    let result = sqlx::query(
        "UPDATE rootcx_system.collection_imports
         SET cancel_requested = true,
             status = CASE WHEN status = 'pending' THEN 'cancelled' ELSE status END,
             completed_at = CASE WHEN status = 'pending' THEN now() ELSE completed_at END,
             token_hash = NULL, token_expires_at = NULL, updated_at = now()
         WHERE id = $1 AND status IN ('pending', 'loading')",
    )
    .bind(id)
    .execute(rt.pool())
    .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::Conflict(format!(
            "cannot cancel import in status {}",
            import.status
        )));
    }
    Ok(Json(json!({ "id": id, "cancel_requested": true })))
}

pub(crate) async fn retry_http(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((app_id, entity, id)): Path<(String, String, Uuid)>,
) -> Result<Json<CollectionImport>, ApiError> {
    let import = find_import(rt.pool(), &app_id, &entity, id).await?;
    if !matches!(import.status.as_str(), STATUS_FAILED | STATUS_CANCELLED) {
        return Err(ApiError::Conflict(format!(
            "cannot retry import in status {}",
            import.status
        )));
    }
    let mode = ImportMode::parse(&import.mode)?;
    let permissions = authorize_control(rt.pool(), &identity, &import).await?;
    require_permissions(&permissions, &app_id, &entity, mode)?;
    source_checksum(rt.pool(), &app_id, import.source_file_id, &permissions).await?;
    let token = secure_tokens::generate();
    let result = sqlx::query(
        "UPDATE rootcx_system.collection_imports
         SET status = 'pending', rows_loaded = 0, bytes_received = 0,
             cancel_requested = false, error = NULL, staging_table = NULL,
             started_at = NULL, completed_at = NULL, token_hash = $2,
             token_expires_at = now() + interval '1 hour', updated_at = now(),
             actor_uid = $3, is_delegated = false, effective_perms = $4
         WHERE id = $1 AND status IN ('failed', 'cancelled')",
    )
    .bind(id)
    .bind(secure_tokens::hash(&token).as_slice())
    .bind(identity.user_id)
    .bind(&permissions)
    .execute(rt.pool())
    .await;
    if let Err(error) = result {
        if is_unique_violation(&error) {
            return Err(ApiError::Conflict(
                "another import is active for this collection".into(),
            ));
        }
        return Err(error.into());
    }
    let mut retried = find_import(rt.pool(), &app_id, &entity, id).await?;
    retried.upload_url = Some(upload_url(rt.runtime_url(), &token));
    Ok(Json(retried))
}
