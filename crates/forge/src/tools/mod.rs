pub mod fs;
pub mod search;
pub mod shell;

use std::path::Path;

use serde_json::{json, Value};

use crate::provider::ToolDef;

pub fn tool_schemas() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "read".into(),
            description: "Read a file's contents. Returns line-numbered text.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "file_path": { "type": "string", "description": "Absolute path to file" },
                    "offset": { "type": "integer", "description": "Start line (1-indexed)" },
                    "limit": { "type": "integer", "description": "Max lines to return" }
                },
                "required": ["file_path"]
            }),
        },
        ToolDef {
            name: "write".into(),
            description: "Write content to a file. Creates parent directories if needed.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "file_path": { "type": "string", "description": "Absolute path to file" },
                    "content": { "type": "string", "description": "File content to write" }
                },
                "required": ["file_path", "content"]
            }),
        },
        ToolDef {
            name: "edit".into(),
            description: "Replace exact string occurrences in a file.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "file_path": { "type": "string", "description": "Absolute path to file" },
                    "old_string": { "type": "string", "description": "Exact text to find" },
                    "new_string": { "type": "string", "description": "Replacement text" }
                },
                "required": ["file_path", "old_string", "new_string"]
            }),
        },
        ToolDef {
            name: "bash".into(),
            description: "Execute a shell command and return stdout/stderr.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Shell command to run" },
                    "timeout": { "type": "integer", "description": "Timeout in seconds (default 120)" }
                },
                "required": ["command"]
            }),
        },
        ToolDef {
            name: "grep".into(),
            description: "Search file contents with regex. Returns matching lines with file paths and line numbers.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Regex pattern" },
                    "path": { "type": "string", "description": "Directory or file to search" },
                    "include": { "type": "string", "description": "Glob filter (e.g. '*.rs')" }
                },
                "required": ["pattern"]
            }),
        },
        ToolDef {
            name: "glob".into(),
            description: "Find files matching a glob pattern.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Glob pattern (e.g. 'src/**/*.rs')" }
                },
                "required": ["pattern"]
            }),
        },
        ToolDef {
            name: "ls".into(),
            description: "List directory contents.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Directory path" }
                },
                "required": ["path"]
            }),
        },
    ]
}

pub async fn execute(name: &str, args: Value, cwd: &Path) -> Result<String, String> {
    match name {
        "read" => fs::read(args, cwd).await,
        "write" => fs::write(args, cwd).await,
        "edit" => fs::edit(args, cwd).await,
        "bash" => shell::bash(args, cwd).await,
        "grep" => search::grep(args, cwd).await,
        "glob" => fs::glob_files(args, cwd).await,
        "ls" => fs::ls(args, cwd).await,
        _ => Err(format!("unknown tool: {name}")),
    }
}

pub fn needs_permission(name: &str) -> bool {
    matches!(name, "write" | "edit" | "bash")
}
