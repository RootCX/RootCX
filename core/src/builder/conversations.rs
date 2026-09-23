use super::*;

pub(super) async fn list(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app): Path<String>,
) -> Result<Json<Value>, ApiError> {
    authorize(&rt, &identity, &app).await?;
    let rows: Vec<Value> = sqlx::query_scalar("SELECT jsonb_build_object('id',c.id,'title',c.title,'updatedAt',COALESCE(r.updated_at,c.created_at),'status',r.status,'message',r.message) FROM rootcx_system.source_conversations c LEFT JOIN LATERAL (SELECT status,message,(SELECT max(updated_at) FROM rootcx_system.source_runs WHERE conversation_id=c.id) AS updated_at FROM rootcx_system.source_runs WHERE conversation_id=c.id ORDER BY created_at DESC,id DESC LIMIT 1) r ON true WHERE c.app_id=$1 AND c.user_id=$2 ORDER BY COALESCE(r.updated_at,c.created_at) DESC,c.id LIMIT 100")
        .bind(app).bind(identity.user_id).fetch_all(rt.pool()).await?;
    Ok(Json(json!(rows)))
}

pub(super) async fn messages(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((app, id)): Path<(String, Uuid)>,
) -> Result<Json<Vec<Run>>, ApiError> {
    authorize(&rt, &identity, &app).await?;
    let owned: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM rootcx_system.source_conversations WHERE id=$1 AND app_id=$2 AND user_id=$3)")
        .bind(id).bind(&app).bind(identity.user_id).fetch_one(rt.pool()).await?;
    if !owned {
        return Err(ApiError::NotFound("conversation not found".into()));
    }
    Ok(Json(sqlx::query_as(&format!("SELECT {RUN_COLUMNS} FROM (SELECT * FROM rootcx_system.source_runs WHERE conversation_id=$1 ORDER BY created_at DESC,id DESC LIMIT 100) recent ORDER BY created_at,id"))
        .bind(id).fetch_all(rt.pool()).await?))
}

pub(super) async fn dispatch(rt: &SharedRuntime) -> Result<(), ApiError> {
    // Queued requests survive a restart; execution claims them under database locks.
    let jobs: Vec<(Uuid,String,Uuid,String,String)> = sqlx::query_as("SELECT DISTINCT ON (app_id) id,app_id,requested_by,base_commit,prompt FROM rootcx_system.source_runs q WHERE status='queued' AND NOT EXISTS(SELECT 1 FROM rootcx_system.source_runs active WHERE active.app_id=q.app_id AND active.status IN ('coding','publishing','needs_recovery')) ORDER BY app_id,created_at,id LIMIT 4")
        .fetch_all(rt.pool()).await?;
    for (id, app, user, base, prompt) in jobs {
        let Ok(runner) = pipeline::Runner::configured() else {
            break;
        };
        tokio::spawn(pipeline::execute(
            rt.clone(),
            runner,
            id,
            app,
            user,
            base,
            prompt,
        ));
    }
    Ok(())
}

pub(super) async fn claim(
    rt: &SharedRuntime,
    id: Uuid,
    app: &str,
) -> Result<Option<String>, ApiError> {
    let mut tx = rt.pool().begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(73423)")
        .execute(&mut *tx)
        .await?;
    let next: Option<Uuid> = sqlx::query_scalar("SELECT id FROM rootcx_system.source_runs WHERE app_id=$1 AND status='queued' ORDER BY created_at,id LIMIT 1")
        .bind(app).fetch_optional(&mut *tx).await?;
    let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM rootcx_system.source_runs WHERE app_id=$1 AND status IN ('coding','publishing','needs_recovery')) OR (SELECT count(*) FROM rootcx_system.source_runs WHERE status IN ('coding','publishing')) >= 1")
        .bind(app).fetch_one(&mut *tx).await?;
    // The builder defaults to one worker. Keep admission aligned with it.
    if next != Some(id) || blocked {
        return Ok(None);
    }
    let base: String = sqlx::query_scalar(
        "SELECT head_commit FROM rootcx_system.source_projects WHERE app_id=$1 FOR UPDATE",
    )
    .bind(app)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("UPDATE rootcx_system.source_runs SET base_commit=$2,status='coding',phase='understanding',message='Je prends connaissance de votre demande.',updated_at=now() WHERE id=$1 AND status='queued'")
        .bind(id).bind(&base).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(Some(base))
}
