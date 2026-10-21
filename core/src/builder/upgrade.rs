//! Release upgrades merge a new catalog version into an app's source history,
//! keeping its own changes. No model runs: a conflict refuses the upgrade.
use super::{RUN_COLUMNS, Run, authorize, files, pipeline, release};
use crate::{api_error::ApiError, auth::identity::Identity, routes::SharedRuntime};
use axum::{
    Json,
    extract::{Path, State},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use files::Files;
use serde::Deserialize;
use std::path::Path as FsPath;
use uuid::Uuid;

/// Tracks the release each upgrade merged, so the next one diffs from it.
const RELEASE_REF: &str = "refs/rootcx/release";

fn pending_ref(id: Uuid) -> String {
    format!("refs/rootcx/pending/{id}")
}

/// Backup then publication, as for a Builder change. The release ref moves only
/// once the merged revision is live; until then the run stays recoverable.
async fn publish(
    rt: &SharedRuntime,
    run: &Run,
    user: Uuid,
    released: &str,
    frontend: Vec<u8>,
) -> Result<(), ApiError> {
    let root = rt.data_dir().join("sources").join(&run.app_id);
    let commit = run
        .commit_id
        .as_deref()
        .ok_or_else(|| ApiError::Internal("upgrade has no revision".into()))?;
    let sources = files::snapshot(&root, commit).await?;
    let version = files::manifest(&sources, &run.app_id)?.version;
    let outcome = async {
        pipeline::backup(&client()?, &root, &run.app_id, commit).await?;
        pipeline::finish(
            rt,
            run.id,
            &run.app_id,
            user,
            &run.base_commit,
            commit,
            &sources,
            frontend,
            &format!("L’application est passée à la version {version}."),
        )
        .await?;
        files::git(&root, &["update-ref", RELEASE_REF, released]).await?;
        files::git(&root, &["update-ref", "-d", &pending_ref(run.id)]).await
    }
    .await;
    if outcome.is_err() {
        let _ = sqlx::query("UPDATE rootcx_system.source_runs SET status='needs_recovery',error=$2,message='La mise à jour nécessite une intervention avant de continuer.',updated_at=now() WHERE id=$1")
            .bind(run.id).bind(format!("{:?}", outcome.as_ref().err())).execute(rt.pool()).await;
    }
    outcome.map(|_| ())
}

async fn reload(rt: &SharedRuntime, id: Uuid) -> Result<Json<Run>, ApiError> {
    Ok(Json(
        sqlx::query_as(&format!(
            "SELECT {RUN_COLUMNS} FROM rootcx_system.source_runs WHERE id=$1"
        ))
        .bind(id)
        .fetch_one(rt.pool())
        .await?,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Upgrade {
    request_id: Uuid,
    base_commit: String,
    files: Files,
    frontend: String,
}

/// Paths that never reach the compiled frontend: Core installs the manifest and
/// deploys the backend from sources, so changes there keep a prebuilt UI valid.
pub(super) fn affects_frontend(path: &str) -> bool {
    !(path == "manifest.json"
        || path.starts_with("backend/")
        || path.starts_with("openspec/")
        || path.ends_with(".md"))
}

async fn changed(root: &FsPath, from: &str, to: &str) -> Result<Vec<String>, ApiError> {
    let out = files::git(root, &["diff", "--name-only", "--no-renames", from, to]).await?;
    Ok(String::from_utf8_lossy(&out)
        .lines()
        .map(String::from)
        .collect())
}

async fn rev(root: &FsPath, args: &[&str]) -> Result<String, ApiError> {
    Ok(String::from_utf8_lossy(&files::git(root, args).await?)
        .trim()
        .to_string())
}

async fn worktree(root: &FsPath, name: &str, commit: &str) -> Result<std::path::PathBuf, ApiError> {
    let path = root.join(".git").join("upgrades").join(name);
    tokio::fs::create_dir_all(path.parent().unwrap())
        .await
        .map_err(files::io)?;
    files::git(
        root,
        &[
            "worktree",
            "add",
            "--detach",
            path.to_str().unwrap(),
            commit,
        ],
    )
    .await?;
    Ok(path)
}

/// The deployed UI, re-packed so publication can reactivate it unchanged.
fn deployed_frontend(dir: &FsPath) -> Result<Vec<u8>, ApiError> {
    fn add(
        tar: &mut tar::Builder<flate2::write::GzEncoder<Vec<u8>>>,
        root: &FsPath,
        dir: &FsPath,
    ) -> Result<(), ApiError> {
        for entry in std::fs::read_dir(dir).map_err(files::io)? {
            let entry = entry.map_err(files::io)?;
            let kind = entry.file_type().map_err(files::io)?;
            if kind.is_dir() {
                add(tar, root, &entry.path())?;
            } else if kind.is_file() {
                let name = entry.path().strip_prefix(root).unwrap().to_path_buf();
                tar.append_path_with_name(entry.path(), name)
                    .map_err(files::io)?;
            }
        }
        Ok(())
    }
    let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::fast(),
    ));
    add(&mut tar, dir, dir)?;
    tar.into_inner()
        .map_err(files::io)?
        .finish()
        .map_err(files::io)
}

pub(super) async fn upgrade(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app): Path<String>,
    Json(input): Json<Upgrade>,
) -> Result<Json<Run>, ApiError> {
    authorize(&rt, &identity, &app).await?;
    let manifest = files::manifest(&input.files, &app)?;
    let release_frontend = STANDARD
        .decode(&input.frontend)
        .map_err(|_| ApiError::BadRequest("invalid frontend archive".into()))?;
    release::validate_archive(&release_frontend)?;
    let mut tx = rt.pool().begin().await?;
    // Same serialization as Builder changes: one source mutation at a time.
    sqlx::query("SELECT pg_advisory_xact_lock(73423)")
        .execute(&mut *tx)
        .await?;
    let existing = sqlx::query_as::<_, Run>(&format!("SELECT {RUN_COLUMNS} FROM rootcx_system.source_runs WHERE app_id=$1 AND requested_by=$2 AND request_id=$3"))
        .bind(&app).bind(identity.user_id).bind(input.request_id).fetch_optional(&mut *tx).await?;
    if let Some(run) = existing {
        if run.status != "needs_recovery" {
            return Ok(Json(run));
        }
        // Retrying the same request resumes an interrupted publication.
        let root = rt.data_dir().join("sources").join(&app);
        let released = rev(
            &root,
            &["rev-parse", "--verify", "--quiet", &pending_ref(run.id)],
        )
        .await?;
        let frontend = tokio::fs::read(
            root.join(".git/prepared")
                .join(run.id.to_string())
                .join("frontend.tar.gz"),
        )
        .await
        .map_err(files::io)?;
        tx.commit().await?;
        publish(&rt, &run, identity.user_id, &released, frontend).await?;
        return reload(&rt, run.id).await;
    }
    let head: Option<String> = sqlx::query_scalar(
        "SELECT head_commit FROM rootcx_system.source_projects WHERE app_id=$1 FOR UPDATE",
    )
    .bind(&app)
    .fetch_optional(&mut *tx)
    .await?;
    let head = head
        .ok_or_else(|| ApiError::NotFound("application sources have not been imported".into()))?;
    if head != input.base_commit {
        return Err(ApiError::Conflict(
            "application version changed; reload its current revision".into(),
        ));
    }
    let busy: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM rootcx_system.source_runs WHERE app_id=$1 AND status IN ('queued','coding','publishing','needs_recovery'))")
        .bind(&app).fetch_one(&mut *tx).await?;
    if busy {
        return Err(ApiError::Conflict(
            "an application change is already active or needs recovery".into(),
        ));
    }
    let root = rt.data_dir().join("sources").join(&app);
    // Imported catalog sources are the root commit until a first upgrade records its release.
    let previous = match rev(&root, &["rev-parse", "--verify", "--quiet", RELEASE_REF]).await {
        Ok(commit) => commit,
        Err(_) => {
            let roots = rev(&root, &["rev-list", "--max-parents=0", &head]).await?;
            let mut roots = roots.lines();
            match (roots.next(), roots.next()) {
                (Some(commit), None) => commit.to_string(),
                _ => {
                    return Err(ApiError::Conflict(
                        "source history has no single release origin".into(),
                    ));
                }
            }
        }
    };
    if files::snapshot(&root, &previous).await? == input.files {
        return Err(ApiError::Conflict("this release is already applied".into()));
    }
    let id = Uuid::new_v4();
    let staged = worktree(&root, &format!("{id}-release"), &previous).await?;
    let merged = async {
        files::git(
            &staged,
            &[
                "rm",
                "-r",
                "--force",
                "--quiet",
                "--ignore-unmatch",
                "--",
                ".",
            ],
        )
        .await?;
        files::write(&staged, &input.files).await?;
        let released =
            files::commit(&staged, &format!("Release {} {}", app, manifest.version)).await?;
        let work = worktree(&root, &format!("{id}-merge"), &head).await?;
        let result = async {
            if files::git(
                &work,
                &[
                    "merge",
                    "--no-ff",
                    "--no-edit",
                    "-m",
                    &format!("Upgrade {} to {}", app, manifest.version),
                    &released,
                ],
            )
            .await
            .is_err()
            {
                let conflicts = files::git(&work, &["diff", "--name-only", "--diff-filter=U"])
                    .await
                    .unwrap_or_default();
                let _ = files::git(&work, &["merge", "--abort"]).await;
                let conflicts = String::from_utf8_lossy(&conflicts)
                    .lines()
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(ApiError::Conflict(format!(
                    "release conflicts with application changes: {conflicts}"
                )));
            }
            Ok::<_, ApiError>((released.clone(), rev(&work, &["rev-parse", "HEAD"]).await?))
        }
        .await;
        let _ = files::git(
            &root,
            &["worktree", "remove", "--force", work.to_str().unwrap()],
        )
        .await;
        result
    }
    .await;
    let _ = files::git(
        &root,
        &["worktree", "remove", "--force", staged.to_str().unwrap()],
    )
    .await;
    let (released, commit) = merged?;
    // The release's UI is exact when the app never touched UI inputs; the deployed
    // UI is exact when the release did not. Otherwise a build is required.
    let frontend = if !changed(&root, &previous, &head)
        .await?
        .iter()
        .any(|p| affects_frontend(p))
    {
        release_frontend
    } else if !changed(&root, &previous, &released)
        .await?
        .iter()
        .any(|p| affects_frontend(p))
    {
        deployed_frontend(
            &rt.data_dir()
                .join("frontends")
                .join(&app)
                .canonicalize()
                .map_err(files::io)?,
        )?
    } else {
        return Err(ApiError::Conflict(
            "release and application both change the interface; a rebuild is required".into(),
        ));
    };
    files::manifest(&files::snapshot(&root, &commit).await?, &app)?;
    files::git(
        &root,
        &["update-ref", &format!("refs/changes/{id}"), &commit],
    )
    .await?;
    files::git(&root, &["update-ref", &pending_ref(id), &released]).await?;
    let prepared = root.join(".git").join("prepared").join(id.to_string());
    tokio::fs::create_dir_all(&prepared)
        .await
        .map_err(files::io)?;
    tokio::fs::write(prepared.join("frontend.tar.gz"), &frontend)
        .await
        .map_err(files::io)?;
    let prompt = format!("Mise à jour vers la version {}", manifest.version);
    sqlx::query("INSERT INTO rootcx_system.source_runs(id,app_id,requested_by,request_id,base_commit,prompt,status,phase,commit_id,message) VALUES($1,$2,$3,$4,$5,$6,'publishing','publishing',$7,'J’applique et vérifie votre modification.')")
        .bind(id).bind(&app).bind(identity.user_id).bind(input.request_id).bind(&head).bind(&prompt).bind(&commit)
        .execute(&mut *tx).await?;
    tx.commit().await?;
    let run: Run = sqlx::query_as(&format!(
        "SELECT {RUN_COLUMNS} FROM rootcx_system.source_runs WHERE id=$1"
    ))
    .bind(id)
    .fetch_one(rt.pool())
    .await?;
    publish(&rt, &run, identity.user_id, &released, frontend).await?;
    reload(&rt, id).await
}

// Same transport as the source import next to it, including the builder CA.
fn client() -> Result<reqwest::Client, ApiError> {
    pipeline::http_client(std::time::Duration::from_secs(120))
}
