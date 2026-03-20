pub mod fs;
pub mod search;
pub mod shell;
pub mod web;

use std::path::Path;
use std::sync::Arc;

use serde_json::{json, Value};

use crate::provider::ToolDef;

pub type ProgressFn = Arc<dyn Fn(&str) + Send + Sync>;

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
        ToolDef {
            name: "web_fetch".into(),
            description: "Fetch a URL and return its contents as readable text. HTML pages are converted to plain text.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "URL to fetch" },
                    "max_length": { "type": "integer", "description": "Max output length in chars (default 30000)" }
                },
                "required": ["url"]
            }),
        },
        ToolDef {
            name: "list_integrations".into(),
            description: "List installed integrations with their actions and parameter schemas. Use this to discover what external services (email, CRM, messaging, etc.) are available before generating code that uses the useIntegration hook.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
        },
        ToolDef {
            name: "question".into(),
            description: "Ask the user questions to gather preferences, clarify instructions, or get decisions on implementation choices.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "questions": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "question": { "type": "string", "description": "The question to ask" },
                                "header": { "type": "string", "description": "Short label (max 12 chars)" },
                                "options": {
                                    "type": "array",
                                    "items": {
                                        "type": "object",
                                        "properties": {
                                            "label": { "type": "string" },
                                            "description": { "type": "string" }
                                        },
                                        "required": ["label", "description"]
                                    }
                                },
                                "multiple": { "type": "boolean", "description": "Allow selecting multiple options" },
                                "custom": { "type": "boolean", "description": "Allow custom text input" }
                            },
                            "required": ["question", "header", "options"]
                        }
                    }
                },
                "required": ["questions"]
            }),
        },
    ]
}

pub async fn execute(name: &str, args: Value, cwd: &Path, on_progress: Option<ProgressFn>) -> Result<String, String> {
    match name {
        "read" => fs::read(args, cwd).await,
        "write" => fs::write(args, cwd).await,
        "edit" => fs::edit(args, cwd).await,
        "bash" => shell::bash(args, cwd, on_progress).await,
        "grep" => search::grep(args, cwd).await,
        "glob" => fs::glob_files(args, cwd).await,
        "ls" => fs::ls(args, cwd).await,
        "web_fetch" => web::fetch(args, cwd).await,
        _ => Err(format!("unknown tool: {name}")),
    }
}

pub fn needs_permission(name: &str) -> bool {
    matches!(name, "write" | "edit" | "bash" | "web_fetch")
}
