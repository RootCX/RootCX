use super::{
    files::{self, Files},
    release,
};
use crate::{api_error::ApiError, routes::SharedRuntime};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;
use uuid::Uuid;

pub struct Runner {
    url: String,
    token: String,
}
impl Runner {
    pub fn configured() -> Result<Self, ApiError> {
        let url = std::env::var("ROOTCX_BUILDER_URL")
            .map_err(|_| ApiError::Unavailable("application builder is not configured".into()))?;
        let parsed = url::Url::parse(&url)
            .map_err(|_| ApiError::Unavailable("invalid builder URL".into()))?;
        if !matches!(parsed.scheme(), "http" | "https")
            || !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.query().is_some()
            || parsed.fragment().is_some()
        {
            return Err(ApiError::Unavailable("invalid builder URL".into()));
        }
        let token = std::env::var("ROOTCX_BUILDER_TOKEN")
            .ok()
            .filter(|s| s.len() >= 32)
            .ok_or_else(|| {
                ApiError::Unavailable("builder authentication is not configured".into())
            })?;
        Ok(Self { url, token })
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Built {
    files: Files,
    frontend: String,
    summary: String,
}

pub async fn execute(
    rt: SharedRuntime,
    runner: Runner,
    id: Uuid,
    app: String,
    user: Uuid,
    base: String,
    prompt: String,
) {
    // The transaction owns a cross-process lease; cancellation/connection loss releases it.
    let mut lease = match rt.pool().begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!(error=?e,"builder lease unavailable");
            return;
        }
    };
    if sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,73422))")
        .bind(id.to_string())
        .execute(&mut *lease)
        .await
        .is_err()
    {
        return;
    }
    let result = tokio::select! {
        result = tokio::time::timeout(
        Duration::from_secs(1200),
        process(&rt, runner, id, &app, user, &base, &prompt),
    )
    => result,
        _ = rt.shutdown_token().cancelled() => Ok(Err(ApiError::Unavailable("Core is shutting down".into()))),
        error = watch_lease(&mut lease) => Ok(Err(error)),
    };
    let error = match result {
        Ok(Ok(())) => return,
        Ok(Err(e)) => format!("{e:?}"),
        Err(_) => "application change timed out".into(),
    };
    tracing::error!(run_id=%id, app_id=%app, error=%error, "application change failed");
    let message = if error.contains("AI_CREDITS_EXHAUSTED") {
        "Votre réserve de crédits IA est insuffisante pour terminer cette modification. Rechargez-la puis réessayez. Votre application n’a pas été modifiée."
    } else {
        "La modification n’a pas pu être terminée. Votre version publiée est conservée."
    };
    let _ = sqlx::query("UPDATE rootcx_system.source_runs SET status=CASE WHEN status='publishing' THEN 'needs_recovery' ELSE 'failed' END,error=$2,message=CASE WHEN status='publishing' THEN 'La mise à jour nécessite une intervention avant de continuer.' ELSE $3 END,updated_at=now() WHERE id=$1 AND status<>'succeeded'")
        .bind(id).bind(error).bind(message).execute(rt.pool()).await;
}

async fn watch_lease(lease: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> ApiError {
    loop {
        tokio::time::sleep(Duration::from_secs(15)).await;
        match tokio::time::timeout(
            Duration::from_secs(10),
            sqlx::query("SELECT 1").execute(&mut **lease),
        )
        .await
        {
            Ok(Ok(_)) => {}
            _ => return ApiError::Unavailable("application change lease was lost".into()),
        }
    }
}

async fn state(rt: &SharedRuntime, id: Uuid, status: &str, message: &str) -> Result<(), ApiError> {
    sqlx::query(
        "UPDATE rootcx_system.source_runs SET status=$2,message=$3,updated_at=now() WHERE id=$1",
    )
    .bind(id)
    .bind(status)
    .bind(message)
    .execute(rt.pool())
    .await?;
    Ok(())
}

async fn process(
    rt: &SharedRuntime,
    runner: Runner,
    id: Uuid,
    app: &str,
    user: Uuid,
    base: &str,
    prompt: &str,
) -> Result<(), ApiError> {
    state(rt, id, "coding", "J’adapte votre application.").await?;
    let root = rt.data_dir().join("sources").join(app);
    let before = files::snapshot(&root, base).await?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(1000))
        .build()
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let mut response = client
        .post(format!("{}/build", runner.url.trim_end_matches('/')))
        .bearer_auth(&runner.token)
        .json(&json!({"runId":id,"appId":app,"baseCommit":base,"prompt":prompt,"files":before}))
        .send()
        .await
        .map_err(|e| ApiError::Unavailable(format!("builder transport: {e}")))?;
    if response.status() == reqwest::StatusCode::PAYMENT_REQUIRED {
        return Err(ApiError::Unavailable("AI_CREDITS_EXHAUSTED".into()));
    }
    if !response.status().is_success() {
        return Err(ApiError::Unavailable(format!(
            "builder returned {}",
            response.status()
        )));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| ApiError::Unavailable(e.to_string()))?
    {
        if bytes.len() + chunk.len() > 160 * 1024 * 1024 {
            return Err(ApiError::BadRequest("build output too large".into()));
        }
        bytes.extend_from_slice(&chunk);
    }
    let built: Built = serde_json::from_slice(&bytes)
        .map_err(|e| ApiError::BadRequest(format!("invalid builder response: {e}")))?;
    let next = files::manifest(&built.files, app)?;
    release::compatible(&files::manifest(&before, app)?, &next)?;
    if before == built.files {
        return Err(ApiError::BadRequest(
            "agent produced no source change".into(),
        ));
    }
    let frontend = STANDARD
        .decode(&built.frontend)
        .map_err(|_| ApiError::BadRequest("invalid frontend archive".into()))?;
    release::validate_archive(&frontend)?;
    let workspace = root.join(".git").join("builds").join(id.to_string());
    tokio::fs::create_dir_all(workspace.parent().unwrap())
        .await
        .map_err(files::io)?;
    files::git(
        &root,
        &[
            "worktree",
            "add",
            "--detach",
            workspace.to_str().unwrap(),
            base,
        ],
    )
    .await?;
    // Clear tracked files only; the Core-owned .git worktree link remains untouched.
    files::git(
        &workspace,
        &["rm", "-r", "--force", "--ignore-unmatch", "--", "."],
    )
    .await?;
    files::write(&workspace, &built.files).await?;
    let commit = files::commit(&workspace, &format!("SHAPP change {id}")).await?;
    files::git(
        &root,
        &["update-ref", &format!("refs/changes/{id}"), &commit],
    )
    .await?;
    let prepared = root.join(".git").join("prepared").join(id.to_string());
    tokio::fs::create_dir_all(&prepared)
        .await
        .map_err(files::io)?;
    tokio::fs::write(prepared.join("frontend.tar.gz"), &frontend)
        .await
        .map_err(files::io)?;
    sqlx::query("UPDATE rootcx_system.source_runs SET commit_id=$2,updated_at=now() WHERE id=$1")
        .bind(id)
        .bind(&commit)
        .execute(rt.pool())
        .await?;
    backup(&client, &root, app, &commit).await?;
    // Re-check live authority after the potentially long model/build phase.
    if !crate::auth::identity::principal_enabled(rt.pool(), user).await {
        return Err(ApiError::Forbidden("requester is no longer active".into()));
    }
    for perm in ["admin:apps.deploy", "admin:apps.install"] {
        crate::governance::authority::require_perm(rt.pool(), user, perm).await?;
    }
    state(
        rt,
        id,
        "publishing",
        "J’applique et vérifie votre modification.",
    )
    .await?;
    finish(
        rt,
        id,
        app,
        user,
        base,
        &commit,
        &built.files,
        frontend,
        &built.summary,
    )
    .await?;
    let _ = files::git(
        &root,
        &["worktree", "remove", "--force", workspace.to_str().unwrap()],
    )
    .await;
    Ok(())
}

async fn finish(
    rt: &SharedRuntime,
    id: Uuid,
    app: &str,
    user: Uuid,
    base: &str,
    commit: &str,
    sources: &Files,
    frontend: Vec<u8>,
    summary: &str,
) -> Result<(), ApiError> {
    let root = rt.data_dir().join("sources").join(app);
    let manifest = files::manifest(sources, app)?;
    release::publish(rt, app, user, &manifest, sources, frontend).await?;
    // Publication recovery may resume after the Git ref moved but before SQL committed.
    let current = String::from_utf8(files::git(&root, &["rev-parse", "main"]).await?).unwrap();
    if current.trim() != commit {
        files::git(&root, &["update-ref", "refs/heads/main", commit, base]).await?;
    }
    files::git(&root, &["reset", "--hard", commit]).await?;
    let mut tx = rt.pool().begin().await?;
    let updated=sqlx::query("UPDATE rootcx_system.source_projects SET head_commit=$2,deployed_commit=$2 WHERE app_id=$1 AND head_commit IN ($2,$3)")
        .bind(app).bind(commit).bind(base).execute(&mut *tx).await?;
    if updated.rows_affected() != 1 {
        return Err(ApiError::Conflict(
            "source revision changed during publication".into(),
        ));
    }
    let summary = if summary.trim().is_empty() {
        "Votre application a été mise à jour."
    } else {
        summary
    };
    sqlx::query("UPDATE rootcx_system.source_runs SET status='succeeded',message=$2,error=NULL,updated_at=now() WHERE id=$1")
        .bind(id).bind(summary.chars().take(2000).collect::<String>()).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn resume(
    rt: SharedRuntime,
    id: Uuid,
    app: String,
    user: Uuid,
    base: String,
    commit: String,
) {
    let recovery = async {
        let mut lease = rt.pool().begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,73422))")
            .bind(id.to_string())
            .execute(&mut *lease)
            .await?;
        let root = rt.data_dir().join("sources").join(&app);
        let sources = files::snapshot(&root, &commit).await?;
        let frontend = tokio::fs::read(
            root.join(".git/prepared")
                .join(id.to_string())
                .join("frontend.tar.gz"),
        )
        .await
        .map_err(files::io)?;
        if !crate::auth::identity::principal_enabled(rt.pool(), user).await {
            return Err(ApiError::Forbidden("requester disabled".into()));
        }
        for perm in ["admin:apps.deploy", "admin:apps.install"] {
            crate::governance::authority::require_perm(rt.pool(), user, perm).await?;
        }
        tokio::select! {
            result = finish(&rt, id, &app, user, &base, &commit, &sources, frontend, "") => result,
            error = watch_lease(&mut lease) => Err(error),
        }
    };
    let result = tokio::select! {
        result = tokio::time::timeout(Duration::from_secs(1200), recovery) =>
            result.unwrap_or_else(|_| Err(ApiError::Unavailable("publication recovery timed out".into()))),
        _ = rt.shutdown_token().cancelled() => Err(ApiError::Unavailable("Core is shutting down".into())),
    };
    if let Err(e) = result {
        tracing::error!(run_id=%id,error=?e,"publication recovery failed");
        let _=sqlx::query("UPDATE rootcx_system.source_runs SET status='needs_recovery',error=$2,updated_at=now() WHERE id=$1").bind(id).bind(format!("{e:?}")).execute(rt.pool()).await;
    }
}

pub(super) async fn backup(
    client: &reqwest::Client,
    root: &std::path::Path,
    app: &str,
    commit: &str,
) -> Result<(), ApiError> {
    let url = match std::env::var("ROOTCX_BUILDER_BACKUP_URL") {
        Ok(url) => url,
        Err(_)
            if std::env::var("ROOTCX_BUILDER_ALLOW_UNBACKED_SOURCES").as_deref() == Ok("true") =>
        {
            return Ok(());
        }
        Err(_) => {
            return Err(ApiError::Unavailable(
                "source backup must be configured before publishing".into(),
            ));
        }
    };
    let url =
        url::Url::parse(&url).map_err(|_| ApiError::Unavailable("invalid backup URL".into()))?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ApiError::Unavailable(
            "backup requires an HTTPS object receiver".into(),
        ));
    }
    let file = root.join(".git").join(format!("{commit}.bundle"));
    files::git(root, &["bundle", "create", file.to_str().unwrap(), "--all"]).await?;
    let bytes = tokio::fs::read(&file).await.map_err(files::io)?;
    let token = std::env::var("ROOTCX_BUILDER_BACKUP_TOKEN")
        .map_err(|_| ApiError::Unavailable("backup authentication missing".into()))?;
    use sha2::{Digest, Sha256};
    let checksum = hex::encode(Sha256::digest(&bytes));
    let expected_size = bytes.len() as u64;
    let response = client
        .put(format!(
            "{}/{app}/{commit}.bundle",
            url.as_str().trim_end_matches('/')
        ))
        .bearer_auth(token)
        .header("content-type", "application/octet-stream")
        .body(bytes)
        .send()
        .await
        .map_err(|e| ApiError::Unavailable(format!("source backup failed: {e}")))?;
    if !response.status().is_success() {
        return Err(ApiError::Unavailable(
            "source backup was not acknowledged".into(),
        ));
    }
    let receipt: serde_json::Value = response
        .json()
        .await
        .map_err(|_| ApiError::Unavailable("invalid backup receipt".into()))?;
    if receipt["sha256"].as_str() != Some(&checksum)
        || receipt["bytes"].as_u64() != Some(expected_size)
    {
        return Err(ApiError::Unavailable(
            "backup checksum was not acknowledged".into(),
        ));
    }
    let _ = tokio::fs::remove_file(file).await;
    Ok(())
}
