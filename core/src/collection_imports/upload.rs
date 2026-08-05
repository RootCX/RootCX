use axum::Json;
use axum::body::Body;
use axum::extract::{Path, State};
use futures::StreamExt;
use serde_json::{Value as JsonValue, json};
use sqlx::{Connection, Executor, PgConnection, PgPool};
use uuid::Uuid;

use super::{ImportMode, STATUS_CANCELLED, STATUS_COMPLETED, STATUS_FAILED, max_import_bytes};
use crate::api_error::ApiError;
use crate::auth::secure_tokens;
use crate::governance::authority::{has_permission, intersect_permissions, resolve_permissions};
use crate::manifest::quote_ident;
use crate::routes::SharedRuntime;

const PROGRESS_BYTES: i64 = 64 * 1024 * 1024;

#[derive(sqlx::FromRow)]
struct ImportExecution {
    id: Uuid,
    app_id: String,
    entity: String,
    mode: String,
    columns: Vec<String>,
    conflict_columns: Vec<String>,
    allow_empty: bool,
    source_file_id: Option<Uuid>,
    source_checksum: Option<String>,
    actor_uid: Uuid,
    is_delegated: bool,
    effective_perms: Vec<String>,
    staging_table: String,
}

fn table(schema: &str, name: &str) -> String {
    format!("{}.{}", quote_ident(schema), quote_ident(name))
}

async fn claim(pool: &PgPool, token: &str) -> Result<ImportExecution, ApiError> {
    if !secure_tokens::is_well_formed(token) {
        return Err(ApiError::NotFound(
            "invalid or expired collection import token".into(),
        ));
    }
    let staging_table = format!("collection_import_{}", Uuid::new_v4().simple());
    sqlx::query_as(
        "UPDATE rootcx_system.collection_imports
         SET status = 'loading', staging_table = $2, started_at = now(),
             updated_at = now(), token_hash = NULL, token_expires_at = NULL
         WHERE token_hash = $1 AND token_expires_at > now() AND status = 'pending'
         RETURNING id, app_id, entity, mode, columns, conflict_columns, allow_empty,
                   source_file_id, source_checksum, actor_uid, is_delegated,
                   effective_perms, staging_table",
    )
    .bind(secure_tokens::hash(token).as_slice())
    .bind(staging_table)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| ApiError::NotFound("invalid or expired collection import token".into()))
}

async fn publication_state(
    pool: &PgPool,
    import: &ImportExecution,
) -> Result<crate::governance::enforcement::ContextState, ApiError> {
    if !crate::auth::identity::principal_enabled(pool, import.actor_uid).await {
        return Err(ApiError::Forbidden("import actor is disabled".into()));
    }
    let current = resolve_permissions(pool, import.actor_uid).await?.1;
    let effective = if import.is_delegated {
        intersect_permissions(&current, &import.effective_perms)
    } else {
        current
    };
    for action in ImportMode::parse(&import.mode)?.required_actions() {
        let required = format!("app:{}:{}.{}", import.app_id, import.entity, action);
        if !has_permission(&effective, &required) {
            return Err(ApiError::Forbidden(format!(
                "permission revoked: {required}"
            )));
        }
    }
    if let Some(file_id) = import.source_file_id {
        let required = format!("app:{}:storage.read", import.app_id);
        if !has_permission(&effective, &required) {
            return Err(ApiError::Forbidden(format!(
                "permission revoked: {required}"
            )));
        }
        let checksum = sqlx::query_as::<_, (Option<String>,)>(
            "SELECT checksum FROM rootcx_system.files WHERE id = $1 AND app_id = $2",
        )
        .bind(file_id)
        .bind(&import.app_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("source file {file_id} no longer exists")))?
        .0;
        if checksum != import.source_checksum {
            return Err(ApiError::Conflict(
                "source file checksum changed after the import was created".into(),
            ));
        }
    }
    Ok(crate::governance::enforcement::ContextState {
        user_id: Some(import.actor_uid),
        is_delegated: import.is_delegated,
        effective_perms: effective,
        connection_id: None,
    })
}

async fn create_staging(conn: &mut PgConnection, import: &ImportExecution) -> Result<(), ApiError> {
    let source = table(&import.app_id, &import.entity);
    let staging = table("pg_temp", &import.staging_table);
    let sql = format!(
        "CREATE TEMP TABLE {staging} (LIKE {source} INCLUDING DEFAULTS INCLUDING GENERATED INCLUDING IDENTITY INCLUDING CONSTRAINTS) ON COMMIT PRESERVE ROWS"
    );
    sqlx::query(&sql).execute(&mut *conn).await?;
    sqlx::query(&format!("GRANT SELECT ON {staging} TO rootcx_app_executor"))
        .execute(conn)
        .await?;
    Ok(())
}

async fn drop_staging(conn: &mut PgConnection, staging_table: &str) {
    let sql = format!("DROP TABLE IF EXISTS {}", table("pg_temp", staging_table));
    if let Err(error) = sqlx::query(&sql).execute(conn).await {
        tracing::warn!(staging_table, %error, "failed to drop collection import staging table");
    }
}

async fn cancelled(pool: &PgPool, id: Uuid) -> Result<bool, ApiError> {
    Ok(sqlx::query_scalar(
        "SELECT cancel_requested FROM rootcx_system.collection_imports WHERE id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await?)
}

async fn mark_failed(pool: &PgPool, import: &ImportExecution, status: &str, error: &str) {
    let message: String = error.chars().take(4000).collect();
    let _ = sqlx::query(
        "UPDATE rootcx_system.collection_imports
         SET status = $2, error = $3, completed_at = now(), updated_at = now()
         WHERE id = $1 AND status IN ('loading', 'publishing')",
    )
    .bind(import.id)
    .bind(status)
    .bind(message)
    .execute(pool)
    .await;
}

async fn copy_body(
    conn: &mut PgConnection,
    control_pool: &PgPool,
    import: &ImportExecution,
    body: Body,
) -> Result<(u64, i64), ApiError> {
    let columns = import
        .columns
        .iter()
        .map(|column| quote_ident(column))
        .collect::<Vec<_>>()
        .join(",");
    let copy_sql = format!(
        "COPY {} ({columns}) FROM STDIN WITH (FORMAT csv, NULL '\\N')",
        table("pg_temp", &import.staging_table),
    );
    let mut copy = conn.copy_in_raw(&copy_sql).await?;
    let mut stream = body.into_data_stream();
    let max_bytes = max_import_bytes();
    let mut bytes_received = 0_i64;
    let mut next_progress = PROGRESS_BYTES;
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(error) => {
                let _ = copy.abort("request body failed").await;
                return Err(ApiError::BadRequest(error.to_string()));
            }
        };
        bytes_received += chunk.len() as i64;
        if bytes_received > max_bytes {
            let _ = copy.abort("collection import size limit exceeded").await;
            return Err(ApiError::BadRequest(format!(
                "collection import exceeds {} bytes",
                max_bytes
            )));
        }
        if bytes_received >= next_progress {
            if cancelled(control_pool, import.id).await? {
                let _ = copy.abort("collection import cancelled").await;
                return Err(ApiError::Conflict("collection import cancelled".into()));
            }
            sqlx::query(
                "UPDATE rootcx_system.collection_imports SET bytes_received = $2, updated_at = now() WHERE id = $1",
            )
            .bind(import.id)
            .bind(bytes_received)
            .execute(control_pool)
            .await?;
            next_progress = bytes_received + PROGRESS_BYTES;
        }
        copy.send(chunk).await?;
    }
    let rows = copy.finish().await?;
    Ok((rows, bytes_received))
}

fn publication_sql(import: &ImportExecution) -> Result<String, ApiError> {
    let target = table(&import.app_id, &import.entity);
    let staging = table("pg_temp", &import.staging_table);
    let columns = import
        .columns
        .iter()
        .map(|column| quote_ident(column))
        .collect::<Vec<_>>();
    let names = columns.join(",");
    let insert = format!("INSERT INTO {target} ({names}) SELECT {names} FROM {staging}");
    match ImportMode::parse(&import.mode)? {
        ImportMode::Append => Ok(insert),
        ImportMode::Replace => Ok(format!("DELETE FROM {target}; {insert}")),
        ImportMode::Upsert => {
            let conflicts = import
                .conflict_columns
                .iter()
                .map(|column| quote_ident(column))
                .collect::<Vec<_>>()
                .join(",");
            let conflict_set: std::collections::HashSet<&str> =
                import.conflict_columns.iter().map(String::as_str).collect();
            let updates = import
                .columns
                .iter()
                .filter(|column| !conflict_set.contains(column.as_str()))
                .map(|column| {
                    let quoted = quote_ident(column);
                    format!("{quoted} = EXCLUDED.{quoted}")
                })
                .collect::<Vec<_>>();
            if updates.is_empty() {
                Ok(format!("{insert} ON CONFLICT ({conflicts}) DO NOTHING"))
            } else {
                Ok(format!(
                    "{insert} ON CONFLICT ({conflicts}) DO UPDATE SET {}",
                    updates.join(",")
                ))
            }
        }
    }
}

async fn publish(
    conn: &mut PgConnection,
    control_pool: &PgPool,
    import: &ImportExecution,
    rows: u64,
    bytes: i64,
) -> Result<(), ApiError> {
    let state = publication_state(control_pool, import).await?;
    let transition = sqlx::query(
        "UPDATE rootcx_system.collection_imports
         SET status = 'publishing', rows_loaded = $2, bytes_received = $3, updated_at = now()
         WHERE id = $1 AND status = 'loading' AND cancel_requested = false",
    )
    .bind(import.id)
    .bind(rows as i64)
    .bind(bytes)
    .execute(control_pool)
    .await?;
    if transition.rows_affected() != 1 {
        return Err(ApiError::Conflict("collection import cancelled".into()));
    }

    let mut tx = conn.begin().await?;
    sqlx::query(&format!(
        "SET LOCAL search_path TO {}, public",
        quote_ident(&import.app_id)
    ))
    .execute(&mut *tx)
    .await?;
    sqlx::query("SET LOCAL statement_timeout = 0")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SET LOCAL lock_timeout = '10s'")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SET LOCAL idle_in_transaction_session_timeout = '10min'")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SELECT set_config('rootcx.bulk_import_id', $1, true)")
        .bind(import.id.to_string())
        .execute(&mut *tx)
        .await?;
    crate::governance::enforcement::set_rls_context(&mut tx, &state).await?;
    crate::extensions::audit::set_context(
        &mut tx,
        Some(import.actor_uid),
        None,
        "collection_import",
    )
    .await?;
    sqlx::query("SET LOCAL ROLE rootcx_app_executor")
        .execute(&mut *tx)
        .await?;
    tx.execute(publication_sql(import)?.as_str()).await?;
    sqlx::query("RESET ROLE").execute(&mut *tx).await?;
    sqlx::query(
        "INSERT INTO rootcx_system.audit_log (
            table_schema, table_name, record_id, operation, new_record,
            actor_uid, trigger_ref
         ) VALUES ($1, $2, $3, 'BULK_IMPORT', $4, $5, 'collection_import')",
    )
    .bind(&import.app_id)
    .bind(&import.entity)
    .bind(import.id.to_string())
    .bind(json!({ "mode": import.mode, "rows": rows, "bytes": bytes }))
    .bind(import.actor_uid)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE rootcx_system.collection_imports
         SET status = 'completed', completed_at = now(), updated_at = now()
         WHERE id = $1 AND status = 'publishing'",
    )
    .bind(import.id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    let analyze = format!("ANALYZE {}", table(&import.app_id, &import.entity));
    if let Err(error) = sqlx::query(&analyze).execute(conn).await {
        tracing::warn!(import_id = %import.id, %error, "collection import analyze failed");
    }
    Ok(())
}

pub(crate) async fn upload(
    State(rt): State<SharedRuntime>,
    Path(token): Path<String>,
    body: Body,
) -> Result<Json<JsonValue>, ApiError> {
    let permit = rt
        .collection_import_slots()
        .acquire()
        .await
        .map_err(|_| ApiError::NotReady)?;
    let import = claim(rt.pool(), &token).await?;
    let mut conn = match rt.collection_import_pool().acquire().await {
        Ok(conn) => conn,
        Err(error) => {
            let message = error.to_string();
            mark_failed(rt.pool(), &import, STATUS_FAILED, &message).await;
            return Err(error.into());
        }
    };

    let result = async {
        publication_state(rt.pool(), &import).await?;
        if cancelled(rt.pool(), import.id).await? {
            return Err(ApiError::Conflict("collection import cancelled".into()));
        }
        create_staging(&mut conn, &import).await?;
        let (rows, bytes) = copy_body(&mut conn, rt.pool(), &import, body).await?;
        if rows == 0 && !import.allow_empty {
            return Err(ApiError::BadRequest(
                "collection import contains no rows; set allowEmpty to publish an empty import"
                    .into(),
            ));
        }
        publish(&mut conn, rt.pool(), &import, rows, bytes).await?;
        Ok::<_, ApiError>(json!({
            "id": import.id,
            "status": STATUS_COMPLETED,
            "rows_loaded": rows,
            "bytes_received": bytes,
        }))
    }
    .await;
    drop_staging(&mut conn, &import.staging_table).await;
    let _ = sqlx::query(
        "UPDATE rootcx_system.collection_imports SET staging_table = NULL, updated_at = now() WHERE id = $1",
    )
    .bind(import.id)
    .execute(rt.pool())
    .await;
    drop(conn);
    drop(permit);

    match result {
        Ok(value) => Ok(Json(value)),
        Err(error) => {
            let message = match &error {
                ApiError::Conflict(message) if message.contains("cancelled") => {
                    mark_failed(rt.pool(), &import, STATUS_CANCELLED, message).await;
                    return Err(error);
                }
                ApiError::NotFound(message)
                | ApiError::BadRequest(message)
                | ApiError::Unauthorized(message)
                | ApiError::Forbidden(message)
                | ApiError::Conflict(message)
                | ApiError::Internal(message) => message.clone(),
                ApiError::NotReady => "runtime not ready".into(),
            };
            mark_failed(rt.pool(), &import, STATUS_FAILED, &message).await;
            Err(error)
        }
    }
}
