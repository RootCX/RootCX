use std::fs;
use std::path::Path;

fn main() {
    tauri_build::build();
    generate_tools_md();
}

fn generate_tools_md() {
    let tools_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../runtime/agent/src/tools");
    let mut explicit = Vec::new();
    let mut implicit = Vec::new();

    if let Ok(dir) = fs::read_dir(&tools_dir) {
        for entry in dir.flatten() {
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) { continue; }
            let meta_path = entry.path().join("meta.json");
            if let Ok(content) = fs::read_to_string(&meta_path) {
                println!("cargo:rerun-if-changed={}", meta_path.display());
                if json_bool(&content, "implicit") {
                    implicit.push(format!("- `{}`: {}",
                        json_str(&content, "name"), json_str(&content, "description")));
                } else {
                    explicit.push(format!("- {} → `{}`: {} {}",
                        json_str(&content, "accessEntity"), json_str(&content, "name"),
                        json_str(&content, "description"), json_str(&content, "whenToUse")));
                }
            }
        }
    }

    println!("cargo:rerun-if-changed={}", tools_dir.display());
    let out = Path::new(&std::env::var("OUT_DIR").unwrap()).join("agent-tools.md");
    fs::write(out, format!(
        "# Agent Tools\n\n\
        Unlisted in `access` = denied.\n\n\
        Explicit — add `{{\"entity\":\"<accessEntity>\",\"actions\":[]}}` to access[]:\n\
        {}\n\n\
        Implicit — auto-enabled when any data entity is in access, never add tool: entry:\n\
        {}\n",
        explicit.join("\n"), implicit.join("\n"))).unwrap();
}

fn json_str<'a>(json: &'a str, key: &str) -> &'a str {
    let needle = format!("\"{key}\"");
    let Some(i) = json.find(&needle) else { return "?" };
    let rest = &json[i + needle.len()..];
    let Some(start) = rest.find('"') else { return "?" };
    let val = &rest[start + 1..];
    let Some(end) = val.find('"') else { return "?" };
    &val[..end]
}

fn json_bool(json: &str, key: &str) -> bool {
    let needle = format!("\"{key}\"");
    json.find(&needle)
        .map(|i| json[i + needle.len()..].trim_start().starts_with(": true"))
        .unwrap_or(false)
}
