use std::path::Path;

use serde_json::Value;

const MAX_LINES: usize = 2000;
const MAX_LINE_LEN: usize = 2000;

pub async fn read(args: Value, cwd: &Path) -> Result<String, String> {
    let file_path = resolve(args["file_path"].as_str().ok_or("missing file_path")?, cwd)?;
    let offset = args["offset"].as_u64().unwrap_or(1).max(1) as usize;
    let limit = args["limit"].as_u64().unwrap_or(MAX_LINES as u64) as usize;

    let content = tokio::fs::read_to_string(&file_path)
        .await
        .map_err(|e| format!("{}: {e}", file_path.display()))?;

    let lines: String = content
        .lines()
        .enumerate()
        .skip(offset - 1)
        .take(limit)
        .map(|(i, line)| {
            let num = i + 1;
            let truncated = if line.len() > MAX_LINE_LEN {
                let mut end = MAX_LINE_LEN;
                while !line.is_char_boundary(end) { end -= 1; }
                &line[..end]
            } else {
                line
            };
            format!("{num:>6}\t{truncated}\n")
        })
        .collect();

    Ok(lines)
}

pub async fn write(args: Value, cwd: &Path) -> Result<String, String> {
    let file_path = resolve(args["file_path"].as_str().ok_or("missing file_path")?, cwd)?;
    let content = args["content"].as_str().ok_or("missing content")?;

    if let Some(parent) = file_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("mkdir: {e}"))?;
    }

    tokio::fs::write(&file_path, content)
        .await
        .map_err(|e| format!("write: {e}"))?;

    Ok(format!("wrote {}", file_path.display()))
}

pub async fn edit(args: Value, cwd: &Path) -> Result<String, String> {
    let file_path = resolve(args["file_path"].as_str().ok_or("missing file_path")?, cwd)?;
    let old = args["old_string"].as_str().ok_or("missing old_string")?;
    let new = args["new_string"].as_str().ok_or("missing new_string")?;

    let content = tokio::fs::read_to_string(&file_path)
        .await
        .map_err(|e| format!("read: {e}"))?;

    let count = content.matches(old).count();
    if count == 0 {
        return Err(format!("old_string not found in {}", file_path.display()));
    }
    if count > 1 {
        return Err(format!(
            "old_string found {count} times in {}; provide more context to make it unique",
            file_path.display()
        ));
    }

    let updated = content.replacen(old, new, 1);
    tokio::fs::write(&file_path, &updated)
        .await
        .map_err(|e| format!("write: {e}"))?;

    Ok(format!("edited {}", file_path.display()))
}

pub async fn glob_files(args: Value, cwd: &Path) -> Result<String, String> {
    let pattern = args["pattern"].as_str().ok_or("missing pattern")?;

    let full_pattern = if Path::new(pattern).is_absolute() {
        pattern.to_string()
    } else {
        format!("{}/{pattern}", cwd.display())
    };

    let entries: Vec<String> = glob::glob(&full_pattern)
        .map_err(|e| format!("invalid glob: {e}"))?
        .filter_map(|r| r.ok())
        .take(200)
        .map(|p| p.display().to_string())
        .collect();

    if entries.is_empty() {
        Ok("no matches".into())
    } else {
        Ok(entries.join("\n"))
    }
}

pub async fn ls(args: Value, cwd: &Path) -> Result<String, String> {
    let path = resolve(args["path"].as_str().ok_or("missing path")?, cwd)?;

    let mut entries = tokio::fs::read_dir(&path)
        .await
        .map_err(|e| format!("{}: {e}", path.display()))?;

    let mut items = Vec::new();
    while let Some(entry) = entries.next_entry().await.map_err(|e| e.to_string())? {
        let name = entry.file_name().to_string_lossy().to_string();
        let meta = entry.metadata().await.ok();
        let suffix = if meta.as_ref().is_some_and(|m| m.is_dir()) {
            "/"
        } else {
            ""
        };
        items.push(format!("{name}{suffix}"));
    }

    items.sort();
    Ok(items.join("\n"))
}

fn resolve(path: &str, cwd: &Path) -> Result<std::path::PathBuf, String> {
    let p = Path::new(path);
    let resolved = if p.is_absolute() { p.to_path_buf() } else { cwd.join(p) };

    let canonical = std::fs::canonicalize(&resolved).unwrap_or_else(|_| {
        let mut out = std::path::PathBuf::new();
        for comp in resolved.components() {
            match comp {
                std::path::Component::ParentDir => { out.pop(); }
                std::path::Component::CurDir => {}
                _ => out.push(comp),
            }
        }
        out
    });

    if let Ok(home) = std::env::var("HOME") {
        let home_path = Path::new(&home);
        if canonical.starts_with(home_path) {
            if let Ok(rel) = canonical.strip_prefix(home_path) {
                let first = rel.components().next()
                    .and_then(|c| c.as_os_str().to_str())
                    .unwrap_or("");
                if matches!(first, ".ssh" | ".gnupg" | ".aws" | ".kube") {
                    return Err(format!("access to ~/{first} is blocked"));
                }
            }
        }
    }

    Ok(canonical)
}
