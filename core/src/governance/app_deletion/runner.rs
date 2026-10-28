use super::{Deletion, event, get};
use crate::{api_error::ApiError, routes::SharedRuntime};
use std::time::Duration;
use uuid::Uuid;

pub fn spawn(rt: SharedRuntime) {
    tokio::spawn(async move {
        let mut ticks = tokio::time::interval(Duration::from_secs(2));
        loop {
            tokio::select! {
                _ = rt.shutdown_token().cancelled() => break,
                _ = ticks.tick() => {
                    if let Err(error) = dispatch(&rt).await {
                        tracing::error!(?error, "application deletion dispatcher failed");
                    }
                }
            }
        }
    });
}

async fn dispatch(rt: &SharedRuntime) -> Result<(), ApiError> {
    let ids: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM rootcx_system.app_deletions WHERE state IN ('deleting','retry') AND retry_at <= now() ORDER BY created_at LIMIT 4")
        .fetch_all(rt.pool()).await?;
    for id in ids {
        // A shutdown may interrupt any step. Its durable phase is retried on boot.
        tokio::select! {
            _ = rt.shutdown_token().cancelled() => break,
            result = execute(rt, id) => {
                if let Err(error) = result { tracing::error!(%id, ?error, "application deletion attempt failed"); }
            }
        }
    }
    Ok(())
}

pub(crate) async fn execute(rt: &SharedRuntime, id: Uuid) -> Result<(), ApiError> {
    let initial = get(rt.pool(), id).await?;
    let _lifecycle =
        super::super::cross_app::lock_app_lifecycle(rt.pool(), &initial.app_id).await?;
    let mut operation = get(rt.pool(), id).await?;
    if !matches!(operation.state.as_str(), "deleting" | "retry") {
        return Ok(());
    }
    let current: Option<Uuid> = sqlx::query_scalar("SELECT id FROM rootcx_system.app_installations WHERE app_id=$1 ORDER BY generation DESC LIMIT 1")
        .bind(&operation.app_id).fetch_optional(rt.pool()).await?;
    if current != Some(operation.installation_id) {
        return Err(ApiError::Conflict("deletion installation changed".into()));
    }
    sqlx::query("UPDATE rootcx_system.app_deletions SET attempts=attempts+1,state='deleting',error=NULL,updated_at=now() WHERE id=$1")
        .bind(id).execute(rt.pool()).await?;
    if let Err(error) = clean(rt, &mut operation).await {
        let detail = format!("{error:?}");
        tracing::error!(%id, phase=%operation.phase, error=%detail, "application deletion will retry");
        let mut tx = rt.pool().begin().await?;
        // Details stay in Core logs. Status and audit never expose row values or secrets.
        sqlx::query("UPDATE rootcx_system.app_deletions SET state='retry',error='Cleanup interrupted; Core will retry',retry_at=now()+make_interval(secs=>LEAST(300,attempts*10)),updated_at=now() WHERE id=$1")
            .bind(id).execute(&mut *tx).await?;
        event(&mut tx, &operation, "retry", Some("Cleanup interrupted")).await?;
        tx.commit().await?;
        return Err(error);
    }
    Ok(())
}

async fn advance(
    rt: &SharedRuntime,
    operation: &mut Deletion,
    phase: &str,
) -> Result<(), ApiError> {
    let mut tx = rt.pool().begin().await?;
    event(&mut tx, operation, "phase_completed", None).await?;
    sqlx::query("UPDATE rootcx_system.app_deletions SET phase=$2,updated_at=now() WHERE id=$1")
        .bind(operation.id)
        .bind(phase)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    operation.phase = phase.into();
    Ok(())
}

async fn remove_directory(rt: &SharedRuntime, app: &str, directory: &str) -> Result<(), ApiError> {
    match tokio::fs::remove_dir_all(rt.data_dir().join(directory).join(app)).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(ApiError::Internal(format!(
            "deletion filesystem cleanup: {error}"
        ))),
    }
}

async fn clean(rt: &SharedRuntime, operation: &mut Deletion) -> Result<(), ApiError> {
    if operation.phase == "quiesce" {
        rt.worker_manager().stop_app(&operation.app_id).await?;
        crate::crons::delete_all_for_app(rt.pool(), &operation.app_id).await?;
        advance(rt, operation, "data").await?;
    }
    if operation.phase == "data" {
        crate::manifest::remove_app_data(rt.pool(), &operation.app_id).await?;
        // DELETE triggers release PostgreSQL large objects as well as metadata.
        for table in ["storage_upload_sessions", "files"] {
            sqlx::query(&format!(
                "DELETE FROM rootcx_system.{table} WHERE app_id=$1"
            ))
            .bind(&operation.app_id)
            .execute(rt.pool())
            .await?;
        }
        advance(rt, operation, "files").await?;
    }
    if operation.phase == "files" {
        for directory in ["apps", "frontends"] {
            remove_directory(rt, &operation.app_id, directory).await?;
        }
        advance(rt, operation, "history").await?;
    }
    if operation.phase == "history" {
        remove_directory(rt, &operation.app_id, "sources").await?;
        let mut tx = rt.pool().begin().await?;
        for table in ["source_runs", "source_conversations", "source_projects"] {
            sqlx::query(&format!(
                "DELETE FROM rootcx_system.{table} WHERE app_id=$1"
            ))
            .bind(&operation.app_id)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query("DELETE FROM rootcx_system.apps WHERE id=$1")
            .bind(&operation.app_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE rootcx_system.app_deletions SET state='succeeded',phase='complete',error=NULL,updated_at=now() WHERE id=$1")
            .bind(operation.id).execute(&mut *tx).await?;
        operation.phase = "complete".into();
        event(&mut tx, operation, "succeeded", None).await?;
        tx.commit().await?;
    }
    Ok(())
}
