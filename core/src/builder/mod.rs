//! Durable source ownership belongs to Core. Coding and builds run out of process.
mod conversations;
mod files;
mod pipeline;
mod release;
#[cfg(test)]
mod tests;

use crate::{api_error::ApiError, auth::identity::Identity, routes::SharedRuntime};
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, State},
    routing::{get, post},
};
use files::Files;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use uuid::Uuid;

pub async fn bootstrap(pool: &PgPool) -> Result<(), crate::RuntimeError> {
    sqlx::raw_sql(include_str!("schema.sql"))
        .execute(pool)
        .await
        .map_err(crate::RuntimeError::Schema)?;
    Ok(())
}

pub fn routes() -> Router<SharedRuntime> {
    Router::new()
        .route(
            "/api/v1/apps/{app_id}/conversations",
            get(conversations::list),
        )
        .route(
            "/api/v1/apps/{app_id}/conversations/{id}",
            get(conversations::messages),
        )
        .route("/api/v1/apps/{app_id}/sources", get(sources).post(import))
        .route("/api/v1/apps/{app_id}/sources/bundle", get(bundle))
        .route("/api/v1/apps/{app_id}/changes", get(history).post(start))
        .route("/api/v1/apps/{app_id}/changes/{id}", get(run))
        .route("/api/v1/apps/{app_id}/changes/{id}/events", get(events))
        .route(
            "/api/v1/apps/{app_id}/changes/{id}/retry-publication",
            post(retry_publication),
        )
        .layer(DefaultBodyLimit::max(96 * 1024 * 1024))
}

async fn authorize(rt: &SharedRuntime, identity: &Identity, app: &str) -> Result<(), ApiError> {
    files::app_id(app)?;
    crate::governance::authority::require_perm(rt.pool(), identity.user_id, "admin:apps.deploy")
        .await?;
    crate::governance::authority::require_perm(rt.pool(), identity.user_id, "admin:apps.install")
        .await?;
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Import {
    files: Files,
}

async fn import(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app): Path<String>,
    Json(input): Json<Import>,
) -> Result<Json<Value>, ApiError> {
    authorize(&rt, &identity, &app).await?;
    let manifest = files::manifest(&input.files, &app)?;
    let installed: Option<Value> =
        sqlx::query_scalar("SELECT manifest FROM rootcx_system.apps WHERE id=$1")
            .bind(&app)
            .fetch_optional(rt.pool())
            .await?
            .flatten();
    let installed = installed.ok_or_else(|| {
        ApiError::NotFound("install the application before importing sources".into())
    })?;
    let installed: rootcx_types::AppManifest =
        serde_json::from_value(installed).map_err(|e| ApiError::Internal(e.to_string()))?;
    if serde_json::to_value(installed).unwrap() != serde_json::to_value(manifest).unwrap() {
        return Err(ApiError::Conflict(
            "source manifest does not match the installed application".into(),
        ));
    }
    let mut lock = rt.pool().begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 73421))")
        .bind(&app)
        .execute(&mut *lock)
        .await?;
    let root = rt.data_dir().join("sources").join(&app);
    if root.exists() {
        // Recover a crash between the filesystem rename and SQL commit without overwriting anything.
        let recorded: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM rootcx_system.source_projects WHERE app_id=$1)",
        )
        .bind(&app)
        .fetch_one(&mut *lock)
        .await?;
        if recorded || files::snapshot(&root, "HEAD").await? != input.files {
            return Err(ApiError::Conflict(
                "sources already exist; import never overwrites a repository".into(),
            ));
        }
        let commit = String::from_utf8(files::git(&root, &["rev-parse", "HEAD"]).await?)
            .unwrap()
            .trim()
            .to_string();
        pipeline::backup(
            &pipeline::http_client(std::time::Duration::from_secs(120))?,
            &root,
            &app,
            &commit,
        )
        .await?;
        sqlx::query("INSERT INTO rootcx_system.source_projects(app_id,head_commit) VALUES($1,$2)")
            .bind(&app)
            .bind(&commit)
            .execute(&mut *lock)
            .await?;
        lock.commit().await?;
        return Ok(Json(
            json!({"appId":app,"headCommit":commit,"deployedCommit":null}),
        ));
    }
    let temp = rt
        .data_dir()
        .join("sources")
        .join(format!(".import-{}", Uuid::new_v4()));
    tokio::fs::create_dir_all(&temp).await.map_err(files::io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(
            temp.parent().unwrap(),
            std::fs::Permissions::from_mode(0o700),
        )
        .await
        .map_err(files::io)?;
    }
    let result = async {
        files::write(&temp, &input.files).await?;
        files::git(&temp, &["init", "--initial-branch=main"]).await?;
        let commit = files::commit(&temp, "Import application sources").await?;
        tokio::fs::rename(&temp, &root).await.map_err(files::io)?;
        pipeline::backup(
            &pipeline::http_client(std::time::Duration::from_secs(120))?,
            &root,
            &app,
            &commit,
        )
        .await?;
        sqlx::query("INSERT INTO rootcx_system.source_projects(app_id,head_commit) VALUES($1,$2)")
            .bind(&app)
            .bind(&commit)
            .execute(&mut *lock)
            .await?;
        lock.commit().await?;
        Ok(Json(
            json!({"appId":app,"headCommit":commit,"deployedCommit":null}),
        ))
    }
    .await;
    let _ = tokio::fs::remove_dir_all(&temp).await;
    result
}

async fn sources(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app): Path<String>,
) -> Result<Json<Value>, ApiError> {
    authorize(&rt, &identity, &app).await?;
    let (head, deployed): (String, Option<String>) = sqlx::query_as(
        "SELECT head_commit,deployed_commit FROM rootcx_system.source_projects WHERE app_id=$1",
    )
    .bind(&app)
    .fetch_optional(rt.pool())
    .await?
    .ok_or_else(|| ApiError::NotFound("application sources have not been imported".into()))?;
    Ok(Json(
        json!({"appId":app,"headCommit":head,"deployedCommit":deployed}),
    ))
}

async fn bundle(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app): Path<String>,
) -> Result<axum::response::Response, ApiError> {
    use axum::response::IntoResponse;
    authorize(&rt, &identity, &app).await?;
    let root = rt.data_dir().join("sources").join(&app);
    let out = root
        .join(".git")
        .join(format!("backup-{}.bundle", Uuid::new_v4()));
    files::git(&root, &["bundle", "create", out.to_str().unwrap(), "--all"]).await?;
    let result = tokio::fs::read(&out).await.map_err(files::io);
    let _ = tokio::fs::remove_file(out).await;
    Ok((
        [
            ("content-type", "application/octet-stream"),
            ("cache-control", "no-store"),
        ],
        result?,
    )
        .into_response())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Change {
    request_id: Uuid,
    conversation_id: Option<Uuid>,
    base_commit: String,
    prompt: String,
}

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
struct Run {
    id: Uuid,
    app_id: String,
    base_commit: String,
    status: String,
    commit_id: Option<String>,
    message: String,
    phase: String,
    conversation_id: Option<Uuid>,
    prompt: String,
    activity: Value,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}
const RUN_COLUMNS: &str = "id,app_id,base_commit,status,commit_id,message,phase,conversation_id,prompt,activity,created_at,updated_at";

async fn start(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app): Path<String>,
    Json(input): Json<Change>,
) -> Result<Json<Run>, ApiError> {
    authorize(&rt, &identity, &app).await?;
    if input.prompt.trim().is_empty()
        || input.prompt.len() > 16000
        || input.base_commit.len() != 40
        || !input.base_commit.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(ApiError::BadRequest("invalid change request".into()));
    }
    let runner = pipeline::Runner::configured()?;
    let mut tx = rt.pool().begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(73423)")
        .execute(&mut *tx)
        .await?;
    let head: Option<String> = sqlx::query_scalar(
        "SELECT head_commit FROM rootcx_system.source_projects WHERE app_id=$1 FOR UPDATE",
    )
    .bind(&app)
    .fetch_optional(&mut *tx)
    .await?;
    let existing = sqlx::query(&format!("SELECT {RUN_COLUMNS},requested_by FROM rootcx_system.source_runs WHERE app_id=$1 AND requested_by=$2 AND request_id=$3"))
        .bind(&app).bind(identity.user_id).bind(input.request_id).fetch_optional(&mut *tx).await?;
    if let Some(row) = existing {
        if row.get::<String, _>("prompt") != input.prompt
            || row.get::<String, _>("base_commit") != input.base_commit
                && input.conversation_id.is_none()
            || input.conversation_id.is_some()
                && row.get::<Option<Uuid>, _>("conversation_id") != input.conversation_id
        {
            return Err(ApiError::Conflict(
                "request ID already used for a different change".into(),
            ));
        }
        return Ok(Json(sqlx::FromRow::from_row(&row)?));
    }
    if head.as_deref() != Some(&input.base_commit) {
        return Err(ApiError::Conflict(
            "application version changed; reload its current revision".into(),
        ));
    }
    let blocked: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM rootcx_system.source_runs WHERE app_id=$1 AND status IN ('queued','coding','publishing','needs_recovery'))")
        .bind(&app).fetch_one(&mut *tx).await?;
    if blocked && input.conversation_id.is_none() {
        return Err(ApiError::Conflict(
            "an application change is already active or needs recovery".into(),
        ));
    }
    let active:i64=sqlx::query_scalar("SELECT count(*) FROM rootcx_system.source_runs WHERE status IN ('queued','coding','publishing')").fetch_one(&mut *tx).await?;
    if active >= 32 {
        return Err(ApiError::Unavailable("builder capacity reached".into()));
    }
    if let Some(conversation) = input.conversation_id {
        sqlx::query("INSERT INTO rootcx_system.source_conversations(id,app_id,user_id,title) VALUES($1,$2,$3,$4) ON CONFLICT(id) DO NOTHING")
            .bind(conversation).bind(&app).bind(identity.user_id).bind(input.prompt.chars().take(90).collect::<String>()).execute(&mut *tx).await?;
        let owned: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM rootcx_system.source_conversations WHERE id=$1 AND app_id=$2 AND user_id=$3)")
            .bind(conversation).bind(&app).bind(identity.user_id).fetch_one(&mut *tx).await?;
        if !owned {
            return Err(ApiError::NotFound("conversation not found".into()));
        }
    }
    let id = Uuid::new_v4();
    let run: Run = sqlx::query_as(&format!("INSERT INTO rootcx_system.source_runs(id,app_id,requested_by,request_id,base_commit,prompt,conversation_id,status,message) VALUES($1,$2,$3,$4,$5,$6,$7,'queued','Votre demande est enregistrée. Je la prends en charge dès que possible.') RETURNING {RUN_COLUMNS}"))
        .bind(id).bind(&app).bind(identity.user_id).bind(input.request_id).bind(&input.base_commit).bind(&input.prompt).bind(input.conversation_id).fetch_one(&mut *tx).await?;
    tx.commit().await?;
    tokio::spawn(pipeline::execute(
        rt,
        runner,
        id,
        app,
        identity.user_id,
        input.base_commit,
        input.prompt,
    ));
    Ok(Json(run))
}

async fn run(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((app, id)): Path<(String, Uuid)>,
) -> Result<Json<Run>, ApiError> {
    authorize(&rt, &identity, &app).await?;
    Ok(Json(
        sqlx::query_as(&format!(
            "SELECT {RUN_COLUMNS} FROM rootcx_system.source_runs WHERE app_id=$1 AND id=$2 AND (conversation_id IS NULL OR requested_by=$3)"
        ))
        .bind(&app)
        .bind(id)
        .bind(identity.user_id)
        .fetch_optional(rt.pool())
        .await?
        .ok_or_else(|| ApiError::NotFound("change not found".into()))?,
    ))
}

async fn events(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((app, id)): Path<(String, Uuid)>,
) -> Result<
    axum::response::Sse<
        impl futures::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>,
    >,
    ApiError,
> {
    use axum::response::sse::{Event, KeepAlive, Sse};
    authorize(&rt, &identity, &app).await?;
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM rootcx_system.source_runs WHERE app_id=$1 AND id=$2 AND (conversation_id IS NULL OR requested_by=$3))",
    )
    .bind(&app)
    .bind(id)
    .bind(identity.user_id)
    .fetch_one(rt.pool())
    .await?;
    if !exists {
        return Err(ApiError::NotFound("change not found".into()));
    }
    // Persisted state is replayed on every connection. Dropping the stream does
    // not cancel the job, and reconnects cannot accidentally start another run.
    let user_id = identity.user_id;
    let stream = futures::stream::unfold(
        (rt, app, id, false, None),
        move |(rt, app, id, done, mut previous)| async move {
            if done {
                return None;
            }
            loop {
                let row: Run = sqlx::query_as(&format!(
                    "SELECT {RUN_COLUMNS} FROM rootcx_system.source_runs WHERE app_id=$1 AND id=$2 AND (conversation_id IS NULL OR requested_by=$3)"
                ))
                .bind(&app)
                .bind(id)
                .bind(user_id)
                .fetch_optional(rt.pool())
                .await
                .ok()??;
                let done = !matches!(row.status.as_str(), "queued" | "coding" | "publishing");
                if previous != Some(row.updated_at) || done {
                    previous = Some(row.updated_at);
                    let event = Event::default().event("change").json_data(row).ok()?;
                    return Some((Ok(event), (rt, app, id, done, previous)));
                }
                tokio::time::sleep(std::time::Duration::from_millis(750)).await;
            }
        },
    );
    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(std::time::Duration::from_secs(10))))
}

async fn history(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app): Path<String>,
) -> Result<Json<Vec<Run>>, ApiError> {
    authorize(&rt, &identity, &app).await?;
    Ok(Json(sqlx::query_as(&format!("SELECT {RUN_COLUMNS} FROM rootcx_system.source_runs WHERE app_id=$1 AND (conversation_id IS NULL OR requested_by=$2) ORDER BY created_at DESC LIMIT 100"))
        .bind(&app).bind(identity.user_id).fetch_all(rt.pool()).await?))
}

pub async fn recover_interrupted(rt: &SharedRuntime) {
    let rt = rt.clone();
    tokio::spawn(async move {
        loop {
            if let Err(e) = recover_once(&rt).await {
                tracing::error!(error=?e,"source recovery scan failed");
            }
            tokio::select! {
                _=tokio::time::sleep(std::time::Duration::from_secs(2))=>{},
                _=rt.shutdown_token().cancelled()=>break,
            }
        }
    });
}

async fn recover_once(rt: &SharedRuntime) -> Result<(), ApiError> {
    conversations::dispatch(rt).await?;
    let ids:Vec<Uuid>=sqlx::query_scalar("SELECT id FROM rootcx_system.source_runs WHERE status IN ('coding','publishing') AND updated_at<now()-interval '2 minutes'")
        .fetch_all(rt.pool()).await?;
    for id in ids {
        let mut tx = rt.pool().begin().await?;
        let free: bool =
            sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtextextended($1,73422))")
                .bind(id.to_string())
                .fetch_one(&mut *tx)
                .await?;
        if free {
            sqlx::query("UPDATE rootcx_system.source_runs SET status=CASE WHEN status='publishing' THEN 'needs_recovery' ELSE 'interrupted' END,message='La modification a été interrompue. Elle n’est pas confirmée comme appliquée.',updated_at=now() WHERE id=$1 AND status IN ('queued','coding','publishing')")
                .bind(id).execute(&mut *tx).await?;
        }
        tx.commit().await?;
    }
    Ok(())
}

async fn retry_publication(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((app, id)): Path<(String, Uuid)>,
) -> Result<Json<Value>, ApiError> {
    authorize(&rt, &identity, &app).await?;
    let row:Option<(String,String)>=sqlx::query_as("UPDATE rootcx_system.source_runs SET status='publishing',resumed_by=$3,message='Je reprends la mise à jour.',updated_at=now() WHERE app_id=$1 AND id=$2 AND status='needs_recovery' AND commit_id IS NOT NULL RETURNING base_commit,commit_id")
        .bind(&app).bind(id).bind(identity.user_id).fetch_optional(rt.pool()).await?;
    let (base, commit) = row
        .ok_or_else(|| ApiError::Conflict("change is not awaiting publication recovery".into()))?;
    tokio::spawn(pipeline::resume(
        rt,
        id,
        app,
        identity.user_id,
        base,
        commit,
    ));
    Ok(Json(json!({"id":id,"status":"publishing"})))
}

pub async fn guard_external_mutation(rt: &SharedRuntime, app: &str) -> Result<(), ApiError> {
    let managed: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM rootcx_system.source_projects WHERE app_id=$1)",
    )
    .bind(app)
    .fetch_one(rt.pool())
    .await?;
    if managed {
        return Err(ApiError::Conflict(
            "this application is source-managed; publish changes through the builder".into(),
        ));
    }
    Ok(())
}
