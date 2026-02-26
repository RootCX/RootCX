use std::path::Path;

use regex::Regex;
use serde_json::Value;

const MAX_MATCHES: usize = 100;
const MAX_LINE_LEN: usize = 2000;

pub async fn grep(args: Value, cwd: &Path) -> Result<String, String> {
    let pattern_str = args["pattern"].as_str().ok_or("missing pattern")?;
    let search_path = args["path"].as_str()
        .map(|p| if Path::new(p).is_absolute() { p.into() } else { cwd.join(p) })
        .unwrap_or_else(|| cwd.to_path_buf());
    let include = args["include"].as_str()
        .map(|g| glob::Pattern::new(g))
        .transpose()
        .map_err(|e| format!("invalid glob: {e}"))?;
    let re = Regex::new(pattern_str).map_err(|e| format!("invalid regex: {e}"))?;

    let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(MAX_MATCHES);

    tokio::task::spawn_blocking(move || {
        search_dir(&search_path, &re, include.as_ref(), &tx, &mut 0);
    });

    let mut results = Vec::new();
    while let Some(line) = rx.recv().await {
        results.push(line);
    }

    if results.is_empty() {
        Ok("no matches".into())
    } else {
        Ok(results.join("\n"))
    }
}

fn search_dir(
    dir: &Path,
    re: &Regex,
    include: Option<&glob::Pattern>,
    tx: &tokio::sync::mpsc::Sender<String>,
    count: &mut usize,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        if *count >= MAX_MATCHES { return; }
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();

        if name.starts_with('.') || name == "node_modules" || name == "target" {
            continue;
        }

        if path.is_dir() {
            search_dir(&path, re, include, tx, count);
        } else if path.is_file() {
            if include.is_some_and(|p| !p.matches(&name)) { continue; }
            search_file(&path, re, tx, count);
        }
    }
}

fn search_file(
    path: &Path,
    re: &Regex,
    tx: &tokio::sync::mpsc::Sender<String>,
    count: &mut usize,
) {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return, // skip binary/unreadable files
    };

    for (i, line) in content.lines().enumerate() {
        if *count >= MAX_MATCHES {
            return;
        }
        if re.is_match(line) {
            let display_line = if line.len() > MAX_LINE_LEN {
                &line[..MAX_LINE_LEN]
            } else {
                line
            };
            let _ = tx.blocking_send(format!(
                "{}:{}:{}",
                path.display(),
                i + 1,
                display_line
            ));
            *count += 1;
        }
    }
}
