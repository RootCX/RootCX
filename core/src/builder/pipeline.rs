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

pub(super) fn http_client(timeout: Duration) -> Result<reqwest::Client, ApiError> {
    let mut client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(timeout);
    if let Ok(path) = std::env::var("ROOTCX_BUILDER_CA_FILE") {
        let pem = std::fs::read(path)
            .map_err(|_| ApiError::Unavailable("builder CA file cannot be read".into()))?;
        let certificates = reqwest::Certificate::from_pem_bundle(&pem)
            .map_err(|_| ApiError::Unavailable("invalid builder CA certificate".into()))?;
        if certificates.is_empty() {
            return Err(ApiError::Unavailable("builder CA bundle is empty".into()));
        }
        for certificate in certificates {
            client = client.add_root_certificate(certificate);
        }
    }
    client.build().map_err(|e| ApiError::Internal(e.to_string()))
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
    _base: String,
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
    let claimed: bool =
        sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtextextended($1,73422))")
            .bind(id.to_string())
            .fetch_one(&mut *lease)
            .await
            .unwrap_or(false);
    if !claimed {
        return;
    }
    let base = match super::conversations::claim(&rt, id, &app).await {
        Ok(Some(base)) => base,
        Ok(None) => return,
        Err(error) => {
            tracing::error!(run_id=%id, error=?error,"builder admission failed");
            return;
        }
    };
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
    let message = failure_message(&error);
    let _ = sqlx::query("UPDATE rootcx_system.source_runs SET status=CASE WHEN status='publishing' THEN 'needs_recovery' ELSE 'failed' END,error=$2,message=CASE WHEN status='publishing' THEN 'La mise à jour nécessite une intervention avant de continuer.' ELSE $3 END,updated_at=now() WHERE id=$1 AND status<>'succeeded'")
        .bind(id).bind(error).bind(message).execute(rt.pool()).await;
}

fn progress_message(phase: &str) -> Option<&'static str> {
    match phase {
        "understanding" => {
            Some("Je regarde comment votre application fonctionne pour préparer votre demande.")
        }
        "editing" => Some("J’adapte votre application à votre demande."),
        "checking" => {
            Some("Je prépare et vérifie les changements pour qu’ils fonctionnent ensemble.")
        }
        "repairing" => Some("J’ai repéré un point à ajuster. Je le corrige avant de continuer."),
        "waiting" => Some(
            "Le service répond plus lentement que prévu. Je patiente et reprends automatiquement.",
        ),
        _ => None,
    }
}

pub(super) fn failure_message(error: &str) -> &'static str {
    let cases = [
        (
            "AI_CREDITS_EXHAUSTED",
            "Votre réserve de crédits IA est insuffisante. Rechargez-la puis relancez votre demande. Votre application n’a pas été modifiée.",
        ),
        (
            "AI_CONFIGURATION",
            "Shappy ne peut pas se connecter au service IA. La connexion doit être rétablie avant de relancer votre demande.",
        ),
        (
            "AI_UNAVAILABLE",
            "Le service IA est momentanément indisponible. Réessayez dans un instant. Votre application n’a pas été modifiée.",
        ),
        (
            "BUILDER_BUSY",
            "Shappy traite déjà d’autres modifications. Réessayez dans un instant.",
        ),
        (
            "AI_LIMIT_REACHED",
            "Shappy n’a pas réussi à terminer cette demande en une seule fois. Essayez de la décomposer en étapes.",
        ),
        (
            "BUILD_TIMEOUT",
            "La modification a pris trop de temps et a été interrompue. Vous pouvez relancer votre demande.",
        ),
        (
            "timed out",
            "La modification a pris trop de temps et a été interrompue. Vous pouvez relancer votre demande.",
        ),
        (
            "builder transport",
            "Le service de modification est momentanément injoignable. Réessayez dans un instant.",
        ),
        (
            "BUILD_FAILED",
            "La modification proposée contient une erreur et n’a pas passé les vérifications. Votre application n’a pas été modifiée. Vous pouvez relancer votre demande pour que Shappy la corrige.",
        ),
        (
            "agent produced no source change",
            "Shappy n’a apporté aucun changement à l’application. Précisez le résultat attendu et réessayez.",
        ),
    ];
    cases.iter().find_map(|(code, message)| error.contains(code).then_some(*message))
        .unwrap_or("Une erreur interne a empêché de terminer la modification. Votre application n’a pas été modifiée. Vous pouvez relancer votre demande ; le diagnostic a été enregistré.")
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
        "UPDATE rootcx_system.source_runs SET status=$2,phase=$2,message=$3,updated_at=now() WHERE id=$1",
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
    let client = http_client(Duration::from_secs(1000))?;
    let conversation: Option<Uuid> =
        sqlx::query_scalar("SELECT conversation_id FROM rootcx_system.source_runs WHERE id=$1")
            .bind(id)
            .fetch_one(rt.pool())
            .await?;
    let mut response = client
        .post(format!("{}/build", runner.url.trim_end_matches('/')))
        .bearer_auth(&runner.token)
        .header("accept", "application/x-ndjson")
        .json(&json!({"runId":id,"appId":app,"baseCommit":base,"prompt":prompt,"conversationId":conversation,"files":before}))
        .send()
        .await
        .map_err(|e| ApiError::Unavailable(format!("builder transport: {e}")))?;
    if response.status() == reqwest::StatusCode::PAYMENT_REQUIRED {
        return Err(ApiError::Unavailable("AI_CREDITS_EXHAUSTED".into()));
    }
    if !response.status().is_success() {
        let status = response.status();
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| ApiError::Unavailable(e.to_string()))?
        {
            if body.len() + chunk.len() > 4096 {
                break;
            }
            body.extend_from_slice(&chunk);
        }
        // Keep only known codes; runner diagnostics may contain source or credentials.
        let payload: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
        let code = match payload["error"].as_str().unwrap_or("") {
            code @ ("AI_UNAVAILABLE" | "AI_CONFIGURATION" | "AI_LIMIT_REACHED" | "BUILD_FAILED"
            | "BUILD_TIMEOUT") => code,
            _ if status == reqwest::StatusCode::SERVICE_UNAVAILABLE => "BUILDER_BUSY",
            _ if status == reqwest::StatusCode::GATEWAY_TIMEOUT => "BUILD_TIMEOUT",
            _ => "BUILD_FAILED",
        };
        return Err(ApiError::Unavailable(code.into()));
    }
    let streaming = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/x-ndjson"));
    let mut bytes = Vec::new();
    let mut scanned = 0;
    let mut completed = None;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| ApiError::Unavailable(format!("builder transport: {e}")))?
    {
        if bytes.len() + chunk.len() > 160 * 1024 * 1024 {
            return Err(ApiError::BadRequest("build output too large".into()));
        }
        bytes.extend_from_slice(&chunk);
        if streaming {
            while let Some(offset) = bytes[scanned..].iter().position(|b| *b == b'\n') {
                let end = scanned + offset;
                let event: serde_json::Value = serde_json::from_slice(&bytes[..end])
                    .map_err(|e| ApiError::BadRequest(format!("invalid builder event: {e}")))?;
                match event["type"].as_str() {
                    Some("progress") if completed.is_none() => {
                        if let Some(fallback) =
                            progress_message(event["phase"].as_str().unwrap_or(""))
                        {
                            let message = event["message"]
                                .as_str()
                                .filter(|s| !s.is_empty() && s.len() <= 1200)
                                .unwrap_or(fallback);
                            sqlx::query("UPDATE rootcx_system.source_runs SET phase=$2,message=$3,updated_at=now() WHERE id=$1 AND status='coding'")
                                .bind(id).bind(event["phase"].as_str()).bind(message).execute(rt.pool()).await?;
                        }
                    }
                    Some("result") if completed.is_none() => {
                        let mut result = event;
                        result.as_object_mut().unwrap().remove("type");
                        completed = Some(serde_json::from_value::<Built>(result).map_err(|e| {
                            ApiError::BadRequest(format!("invalid builder result: {e}"))
                        })?);
                    }
                    Some("error") => {
                        let code = match event["error"].as_str().unwrap_or("") {
                            code @ ("AI_CREDITS_EXHAUSTED"
                            | "AI_UNAVAILABLE"
                            | "AI_CONFIGURATION"
                            | "AI_LIMIT_REACHED"
                            | "BUILD_FAILED"
                            | "BUILD_TIMEOUT") => code,
                            _ => "BUILD_FAILED",
                        };
                        return Err(ApiError::Unavailable(code.into()));
                    }
                    _ => return Err(ApiError::BadRequest("invalid builder event order".into())),
                }
                bytes.drain(..=end);
                scanned = 0;
            }
            scanned = bytes.len();
        }
    }
    let built: Built = if streaming {
        if !bytes.is_empty() {
            return Err(ApiError::Unavailable(
                "builder transport ended mid-event".into(),
            ));
        }
        completed
            .ok_or_else(|| ApiError::Unavailable("builder transport ended without result".into()))?
    } else {
        serde_json::from_slice(&bytes)
            .map_err(|e| ApiError::BadRequest(format!("invalid builder response: {e}")))?
    };
    files::manifest(&built.files, app)?;
    if before == built.files {
        sqlx::query("UPDATE rootcx_system.source_runs SET status='succeeded',phase='answered',message=$2,updated_at=now() WHERE id=$1")
            .bind(id).bind(built.summary.chars().take(2000).collect::<String>()).execute(rt.pool()).await?;
        return Ok(());
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
    let local_http = std::env::var("ROOTCX_BUILDER_ALLOW_LOCAL_HTTP").as_deref() == Ok("true")
        && url.scheme() == "http"
        && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    if (url.scheme() != "https" && !local_http)
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
