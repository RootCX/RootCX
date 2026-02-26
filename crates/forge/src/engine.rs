use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use futures::StreamExt;
use serde_json::{json, Value};
use sqlx::PgPool;
use tracing::{info, warn};
use uuid::Uuid;

use crate::error::ForgeError;
use crate::permission::{PendingPermissions, PermissionResponse};
use crate::provider::{
    self, ChatMessage, ContentBlock, LlmProvider, ProviderKind, Role, StopReason, StreamEvent,
    ToolDef,
};
use crate::question::PendingQuestions;
use crate::session;
use crate::tools;

const MAX_TURNS: usize = 50;

pub type EmitFn = Arc<dyn Fn(&str, Value) + Send + Sync>;

pub struct LoopContext {
    pub pool: PgPool,
    pub session_id: Uuid,
    pub cwd: PathBuf,
    pub system_prompt: String,
    pub provider_kind: ProviderKind,
    pub model: String,
    pub api_key: Option<String>,
    pub region: Option<String>,
    pub permissions: Arc<PendingPermissions>,
    pub questions: Arc<PendingQuestions>,
    pub emit: EmitFn,
}

pub async fn agentic_loop(ctx: LoopContext, user_text: &str) -> Result<(), ForgeError> {
    let tool_defs = tools::tool_schemas();

    let mut messages = build_history(&ctx.pool, ctx.session_id).await?;

    let user_msg =
        session::insert_message(&ctx.pool, ctx.session_id, "user").await?;
    let user_part = session::upsert_part(
        &ctx.pool,
        Uuid::new_v4(),
        user_msg.id,
        "text",
        user_text,
        None,
        None,
    )
    .await?;
    (ctx.emit)("forge://message-updated", json!({
        "info": user_msg,
        "parts": [user_part],
    }));

    messages.push(ChatMessage {
        role: Role::User,
        content: vec![ContentBlock::Text {
            text: user_text.to_string(),
        }],
    });

    let provider = provider::build_provider(&ctx.provider_kind, &ctx.model, ctx.api_key.as_deref(), ctx.region.as_deref());

    for turn in 0..MAX_TURNS {
        info!(turn, session_id = %ctx.session_id, "agentic turn");

        let result = run_turn(
            &ctx,
            provider.as_ref(),
            &tool_defs,
            &mut messages,
        )
        .await;

        match result {
            Ok(TurnResult::Done) => break,
            Ok(TurnResult::ContinueWithTools) => continue,
            Err(ForgeError::PermissionRejected) => {
                info!("permission rejected, stopping loop");
                break;
            }
            Err(ForgeError::Aborted) => {
                info!("aborted by user");
                break;
            }
            Err(e) => {
                warn!(error = %e, "agentic loop error");
                (ctx.emit)("forge://error", json!({"error": e.to_string()}));
                break;
            }
        }
    }

    (ctx.emit)("forge://session-idle", json!({"sessionID": ctx.session_id}));
    Ok(())
}

enum TurnResult {
    Done,
    ContinueWithTools,
}

async fn run_turn(
    ctx: &LoopContext,
    provider: &dyn LlmProvider,
    tool_defs: &[ToolDef],
    messages: &mut Vec<ChatMessage>,
) -> Result<TurnResult, ForgeError> {
    let assistant_msg =
        session::insert_message(&ctx.pool, ctx.session_id, "assistant").await?;
    (ctx.emit)("forge://message-updated", json!({"info": assistant_msg}));

    let mut event_stream = provider.stream(&ctx.system_prompt, messages, tool_defs).await?;

    let mut text_buf = String::new();
    let mut reasoning_buf = String::new();
    let text_part_id = Uuid::new_v4();
    let reasoning_part_id = Uuid::new_v4();
    let mut tool_calls: HashMap<String, (String, String)> = HashMap::new(); // id -> (name, json_args)
    let mut stop_reason = StopReason::EndTurn;
    let mut assistant_content: Vec<ContentBlock> = Vec::new();

    while let Some(event) = event_stream.next().await {
        let event = event?;
        match event {
            StreamEvent::TextDelta(t) => {
                text_buf.push_str(&t);
                let part = session::upsert_part(
                    &ctx.pool, text_part_id, assistant_msg.id, "text", &text_buf, None, None,
                ).await?;
                (ctx.emit)("forge://part-updated", json!({"part": part}));
            }
            StreamEvent::ReasoningDelta(t) => {
                reasoning_buf.push_str(&t);
                let part = session::upsert_part(
                    &ctx.pool, reasoning_part_id, assistant_msg.id, "reasoning", &reasoning_buf, None, None,
                ).await?;
                (ctx.emit)("forge://part-updated", json!({"part": part}));
            }
            StreamEvent::ToolCallStart { id, name } => {
                tool_calls.insert(id, (name, String::new()));
            }
            StreamEvent::ToolCallDelta { id, json } => {
                if let Some((_name, buf)) = tool_calls.get_mut(&id) {
                    buf.push_str(&json);
                }
            }
            StreamEvent::ToolCallEnd { id } => {
                if let Some((name, args_json)) = tool_calls.remove(&id) {
                    let args: Value = serde_json::from_str(&args_json).unwrap_or(json!({}));

                    // Record tool_use in assistant content
                    assistant_content.push(ContentBlock::ToolUse {
                        id: id.clone(),
                        name: name.clone(),
                        input: args.clone(),
                    });

                    let tool_part_id = Uuid::new_v4();
                    let title = format_tool_title(&name, &args);
                    let part = session::upsert_part(
                        &ctx.pool, tool_part_id, assistant_msg.id, "tool", "", Some(&name),
                        Some(&json!({"status": "running", "title": title})),
                    ).await?;
                    (ctx.emit)("forge://part-updated", json!({"part": part}));

                    let result = execute_with_permission(ctx, &name, &args).await?;
                    let is_error = result.is_err();
                    let content = result.unwrap_or_else(|e| e);
                    let status = if is_error { "error" } else { "completed" };

                    let part = session::upsert_part(
                        &ctx.pool, tool_part_id, assistant_msg.id, "tool", &content, Some(&name),
                        Some(&json!({"status": status, "title": title})),
                    ).await?;
                    (ctx.emit)("forge://part-updated", json!({"part": part}));

                    messages.push(ChatMessage {
                        role: Role::User,
                        content: vec![ContentBlock::ToolResult {
                            tool_use_id: id, content, is_error,
                        }],
                    });
                }
            }
            StreamEvent::Done(reason) => {
                stop_reason = reason;
            }
            StreamEvent::Error(e) => {
                return Err(ForgeError::Stream(e));
            }
        }
    }

    // Build assistant message content
    if !text_buf.is_empty() {
        assistant_content.insert(
            0,
            ContentBlock::Text {
                text: text_buf,
            },
        );
    }

    // Add assistant message to conversation history
    messages.push(ChatMessage {
        role: Role::Assistant,
        content: assistant_content,
    });

    let completed = session::complete_message(&ctx.pool, assistant_msg.id).await?;
    (ctx.emit)("forge://message-updated", json!({"info": completed}));

    match stop_reason {
        StopReason::ToolUse => Ok(TurnResult::ContinueWithTools),
        _ => Ok(TurnResult::Done),
    }
}

async fn execute_with_permission(
    ctx: &LoopContext,
    tool_name: &str,
    args: &Value,
) -> Result<Result<String, String>, ForgeError> {
    if tools::needs_permission(tool_name)
        && !ctx.permissions.is_allowed(ctx.session_id, tool_name).await
    {
        let desc = format!("{tool_name}: {}", serde_json::to_string(args).unwrap_or_default());
        let (perm, rx) = ctx.permissions.request(ctx.session_id, tool_name, &desc).await;
        (ctx.emit)("forge://permission-updated", serde_json::to_value(&perm).unwrap());

        let response = rx.await.unwrap_or(PermissionResponse::Reject);
        (ctx.emit)(
            "forge://permission-replied",
            json!({"sessionID": ctx.session_id, "permissionID": perm.id}),
        );

        ctx.permissions
            .reply(perm.id, ctx.session_id, tool_name, response.clone())
            .await;

        if response == PermissionResponse::Reject {
            return Err(ForgeError::PermissionRejected);
        }
    }

    Ok(tools::execute(tool_name, args.clone(), &ctx.cwd).await)
}

/// Reconstruct LLM conversation from DB.
/// Tool parts are split: ToolUse in assistant msg, ToolResult in synthetic user msg.
async fn build_history(pool: &PgPool, session_id: Uuid) -> Result<Vec<ChatMessage>, ForgeError> {
    let rows = session::get_messages_with_parts(pool, session_id).await?;
    let mut messages = Vec::new();

    for row in rows {
        match row.info.role.as_str() {
            "user" => {
                let content: Vec<_> = row.parts.iter()
                    .filter(|p| p.part_type == "text")
                    .map(|p| ContentBlock::Text { text: p.content.clone() })
                    .collect();
                if !content.is_empty() {
                    messages.push(ChatMessage { role: Role::User, content });
                }
            }
            "assistant" => {
                let mut content = Vec::new();
                let mut tool_results = Vec::new();
                for p in &row.parts {
                    match p.part_type.as_str() {
                        "text" if !p.content.is_empty() => {
                            content.push(ContentBlock::Text { text: p.content.clone() });
                        }
                        "tool" => {
                            if let Some(name) = &p.tool_name {
                                let id = p.id.to_string();
                                content.push(ContentBlock::ToolUse {
                                    id: id.clone(), name: name.clone(), input: json!({}),
                                });
                                tool_results.push(ContentBlock::ToolResult {
                                    tool_use_id: id,
                                    content: p.content.clone(),
                                    is_error: p.tool_state.as_ref()
                                        .and_then(|s| s["status"].as_str())
                                        .is_some_and(|s| s == "error"),
                                });
                            }
                        }
                        _ => {}
                    }
                }
                if !content.is_empty() {
                    messages.push(ChatMessage { role: Role::Assistant, content });
                }
                if !tool_results.is_empty() {
                    messages.push(ChatMessage { role: Role::User, content: tool_results });
                }
            }
            _ => {}
        }
    }
    Ok(messages)
}

fn format_tool_title(name: &str, args: &Value) -> String {
    match name {
        "read" => format!("Reading {}", args["file_path"].as_str().unwrap_or("file")),
        "write" => format!("Writing {}", args["file_path"].as_str().unwrap_or("file")),
        "edit" => format!("Editing {}", args["file_path"].as_str().unwrap_or("file")),
        "bash" => {
            let cmd = args["command"].as_str().unwrap_or("...");
            format!("Running `{}`", &cmd[..cmd.floor_char_boundary(40)])
        }
        "grep" => format!("Searching for {}", args["pattern"].as_str().unwrap_or("...")),
        "glob" => format!("Finding {}", args["pattern"].as_str().unwrap_or("...")),
        "ls" => format!("Listing {}", args["path"].as_str().unwrap_or("...")),
        _ => name.to_string(),
    }
}
