use super::{str_arg, Tool, ToolContext};
use crate::extensions::storage::backend::{PostgresBackend, StorageBackend};
use async_trait::async_trait;
use command_group::AsyncCommandGroup;
use rootcx_types::ToolDescriptor;
use serde_json::{json, Value};
use std::{
    path::{Component, Path},
    process::Stdio,
    time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command};

const SUPERVISOR: &str = "/opt/rootcx-worker-sandbox/supervisor.mjs";
const MAX_BYTES: usize = 20 * 1024 * 1024;
pub struct ExecuteSandboxTool;

pub(crate) fn available() -> bool {
    cfg!(target_os = "linux")
        && std::env::var("ROOTCX_AGENT_SANDBOX").as_deref() == Ok("srt")
        && Path::new(SUPERVISOR).is_file()
}
fn relative(path: &str) -> Result<&Path, String> {
    let path = Path::new(path);
    if path.as_os_str().is_empty() || !path.components().all(|p| matches!(p, Component::Normal(_)))
    {
        return Err("sandbox files require relative paths without traversal".into());
    }
    Ok(path)
}

#[async_trait]
impl Tool for ExecuteSandboxTool {
    fn requires_agent_context(&self) -> bool { true }
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            name: "execute_sandbox".into(),
            description: "Run a shell command in a fresh, offline sandbox with no credentials. Python 3, Bun, and Poppler PDF tools are available. Input files are explicit text or files this person gave this agent. Write deliverables beneath outputs/; generated files return storage file_id values for business actions. Never use this to bypass business actions or their calculations.".into(),
            input_schema: json!({"type":"object","properties":{
                "command":{"type":"string"},
                "files":{"type":"array","items":{"type":"object","properties":{
                    "path":{"type":"string"},"file_id":{"type":"string"},"text":{"type":"string"}
                },"required":["path"]}}
            },"required":["command"]}),
        }
    }
    async fn execute(&self, ctx: &ToolContext) -> Result<Value, String> {
        let authority = ctx
            .delegated_authority()
            .ok_or("sandbox execution requires agent authority")?;
        if !available() {
            return Err("isolated command execution is unavailable".into());
        }
        let command = str_arg(&ctx.args, "command")?;
        if command.len() > 64_000 {
            return Err("sandbox command too large".into());
        }
        let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
        let workspace = dir.path().join("workspace");
        tokio::fs::create_dir_all(workspace.join("outputs"))
            .await
            .map_err(|e| e.to_string())?;
        let mut total = 0usize;
        if let Some(files) = ctx.args.get("files").and_then(Value::as_array) {
            if files.len() > 50 {
                return Err("too many sandbox input files".into());
            }
            for file in files {
                let path = workspace.join(relative(str_arg(file, "path")?)?);
                let bytes = if let Some(id) = file.get("file_id").and_then(Value::as_str) {
                    let id = id.parse::<uuid::Uuid>().map_err(|e| e.to_string())?;
                    let owned: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM rootcx_system.files WHERE id=$1 AND app_id=$2 AND uploaded_by=$3 AND size <= $4)")
                        .bind(id).bind(authority.app()).bind(authority.human()).bind(MAX_BYTES as i64)
                        .fetch_one(&ctx.pool).await.map_err(|e| e.to_string())?;
                    if !owned {
                        return Err("sandbox input file is not owned by this person and agent, or is too large".into());
                    }
                    PostgresBackend
                        .get(&ctx.pool, id, authority.app())
                        .await
                        .map_err(|e| e.to_string())?
                        .content
                        .to_vec()
                } else {
                    str_arg(file, "text")?.as_bytes().to_vec()
                };
                total += bytes.len();
                if total > MAX_BYTES {
                    return Err("sandbox inputs exceed 20 MiB".into());
                }
                tokio::fs::create_dir_all(path.parent().unwrap())
                    .await
                    .map_err(|e| e.to_string())?;
                tokio::fs::write(path, bytes)
                    .await
                    .map_err(|e| e.to_string())?;
            }
        }
        let bun = std::env::var("BUN_PATH").unwrap_or_else(|_| "/opt/rootcx/resources/bun".into());
        let script = format!("ulimit -f 32768; ulimit -t 30; {command}");
        let spec = json!({"root":workspace,"entryPoint":bun,"args":["/bin/bash","-c",script],"env":{},"writeRoot":true,"timeoutMs":60_000,"offline":true});
        let output_path = dir.path().join("command.log");
        let output = std::fs::File::create(&output_path).map_err(|e| e.to_string())?;
        let mut child = Command::new(bun)
            .env_clear()
            .arg(SUPERVISOR)
            .arg(spec.to_string())
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env("HOME", dir.path())
            .stdin(Stdio::null())
            .stdout(Stdio::from(output.try_clone().map_err(|e| e.to_string())?))
            .stderr(Stdio::from(output))
            .kill_on_drop(true)
            .group_spawn()
            .map_err(|e| e.to_string())?;
        let status = tokio::select! {
            result=tokio::time::timeout(Duration::from_secs(70),child.wait()) => match result {
                Ok(result)=>result.map_err(|e|e.to_string())?,
                Err(_)=>{let _=child.kill().await;return Err("sandbox execution timed out".into())}
            },
            _=authority.cancelled()=>{let _=child.kill().await;return Err("agent invocation ended".into())}
        };
        // Never publish artifacts if authority was withdrawn while code ran.
        authority.revalidate(&ctx.pool).await?;
        let mut bytes = Vec::new();
        tokio::fs::File::open(output_path)
            .await
            .map_err(|e| e.to_string())?
            .take(64_001)
            .read_to_end(&mut bytes)
            .await
            .map_err(|e| e.to_string())?;
        let truncated = bytes.len() > 64_000;
        bytes.truncate(64_000);
        let mut artifacts = vec![];
        let outputs = workspace.join("outputs");
        if tokio::fs::symlink_metadata(&outputs)
            .await
            .map_err(|e| e.to_string())?
            .file_type()
            .is_symlink()
        {
            return Err("sandbox output directory cannot be a symbolic link".into());
        }
        let mut dirs = vec![outputs];
        let mut scanned = 0;
        let mut size = 0;
        while let Some(dir) = dirs.pop() {
            let mut entries = tokio::fs::read_dir(dir).await.map_err(|e| e.to_string())?;
            while let Some(entry) = entries.next_entry().await.map_err(|e| e.to_string())? {
                scanned += 1;
                if scanned > 1000 {
                    return Err("too many sandbox output entries".into());
                }
                let ty = entry.file_type().await.map_err(|e| e.to_string())?;
                if ty.is_symlink() {
                    continue;
                }
                if ty.is_dir() {
                    dirs.push(entry.path());
                    continue;
                }
                if !ty.is_file() {
                    continue;
                }
                size += entry.metadata().await.map_err(|e| e.to_string())?.len();
                if artifacts.len() >= 20 || size > MAX_BYTES as u64 {
                    return Err("sandbox outputs exceed limits".into());
                }
                let content = tokio::fs::read(entry.path())
                    .await
                    .map_err(|e| e.to_string())?;
                let name = entry.file_name().to_string_lossy().to_string();
                let mime = if content.starts_with(b"%PDF-") {
                    "application/pdf"
                } else if content.starts_with(&[137, 80, 78, 71, 13, 10, 26, 10]) {
                    "image/png"
                } else if content.starts_with(&[255, 216, 255]) {
                    "image/jpeg"
                } else if std::str::from_utf8(&content).is_ok() {
                    "text/plain"
                } else {
                    "application/octet-stream"
                };
                let id = uuid::Uuid::new_v4();
                PostgresBackend
                    .put(
                        &ctx.pool,
                        id,
                        authority.app(),
                        &name,
                        mime,
                        &content,
                        Some(authority.human()),
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                artifacts.push(json!({"file_id":id,"name":name,"content_type":mime,"path":entry.path().strip_prefix(&workspace).unwrap().to_string_lossy()}));
            }
        }
        Ok(
            json!({"output":String::from_utf8_lossy(&bytes),"exit_code":status.code().unwrap_or(-1),"truncated":truncated,"artifacts":artifacts}),
        )
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn sandbox_inputs_cannot_escape_the_workspace() {
        for path in [
            "",
            "/etc/passwd",
            "../secrets",
            "inputs/../../secrets",
            "./inputs/file",
        ] {
            assert!(super::relative(path).is_err(), "{path}");
        }
        assert!(super::relative("attachments/devis.pdf").is_ok());
    }
}
