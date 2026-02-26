use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use futures::StreamExt;
use serde_json::{json, Value};
use sqlx::PgPool;
use tracing::{info, warn};
use uuid::Uuid;

use crate::compactor::Compactor;
use crate::error::ForgeError;
use crate::permission::{PendingPermissions, PermissionResponse};
use crate::provider::{
    self, ChatMessage, ContentBlock, LlmProvider, Role, StopReason, StreamEvent, ToolDef,
};
use crate::question::{PendingQuestions, QuestionInfo, QuestionResponse};
use crate::session;
use crate::tools;
use crate::ForgeConfig;

const MAX_TURNS: usize = 50;
const COMPACT_THRESHOLD_PCT: usize = 80;
const COMPACT_KEEP: usize = 4;

pub type EmitFn = Arc<dyn Fn(&str, Value) + Send + Sync>;

pub struct LoopContext {
    pub pool: PgPool,
    pub session_id: Uuid,
    pub cwd: PathBuf,
    pub system_prompt: String,
    pub provider: Box<dyn LlmProvider>,
    pub compactor: Box<dyn Compactor>,
    pub config: ForgeConfig,
    pub permissions: Arc<PendingPermissions>,
    pub questions: Arc<PendingQuestions>,
    pub emit: EmitFn,
}

pub async fn agentic_loop(mut ctx: LoopContext, user_text: &str) -> Result<(), ForgeError> {
    let tool_defs = tools::tool_schemas();

    ctx.system_prompt.push_str(&format!(
        "\n\nWorking directory: {}\nPlatform: {} {}",
        ctx.cwd.display(), std::env::consts::OS, std::env::consts::ARCH,
    ));

    let (mut messages, summary) = build_history(&ctx.pool, ctx.session_id).await?;
    let base_system_prompt = ctx.system_prompt.clone();
    if let Some(ref s) = summary {
        ctx.system_prompt = format!("{base_system_prompt}\n\n[Previous conversation summary]\n{s}");
    }

    let user_msg = session::insert_message(&ctx.pool, ctx.session_id, "user").await?;
    let user_part = session::upsert_part(
        &ctx.pool, Uuid::new_v4(), user_msg.id, "text", user_text, None, None, None,
    ).await?;
    (ctx.emit)("forge://message-updated", json!({
        "info": user_msg,
        "parts": [user_part],
    }));

    if messages.is_empty() {
        tokio::spawn(generate_title(
            ctx.pool.clone(), ctx.session_id, user_text.to_string(), ctx.config.clone(), ctx.emit.clone(),
        ));
    }
    messages.push(ChatMessage {
        role: Role::User,
        content: vec![ContentBlock::Text { text: user_text.to_string() }],
    });

    for turn in 0..MAX_TURNS {
        let estimated = provider::estimate_tokens(&ctx.system_prompt, &messages);
        let window = ctx.provider.context_window();
        if estimated > window * COMPACT_THRESHOLD_PCT / 100 && messages.len() > COMPACT_KEEP + 2 {
            info!(estimated, window, "compacting conversation");
            (ctx.emit)("forge://compacting", json!({"sessionID": ctx.session_id}));

            let keep = COMPACT_KEEP.min(messages.len() - 1);
            let summary_text = ctx.compactor
                .compact(ctx.provider.as_ref(), &messages, keep)
                .await?;

            let summary_msg = session::insert_message(&ctx.pool, ctx.session_id, "user").await?;
            session::upsert_part(
                &ctx.pool, Uuid::new_v4(), summary_msg.id, "text",
                &summary_text, None, None, None,
            ).await?;
            session::complete_message(&ctx.pool, summary_msg.id).await?;
            session::set_summary_message_id(&ctx.pool, ctx.session_id, summary_msg.id).await?;

            messages.drain(..messages.len() - keep);

            ctx.system_prompt = format!(
                "{base_system_prompt}\n\n[Conversation compacted]\n{summary_text}"
            );

            (ctx.emit)("forge://compacted", json!({"sessionID": ctx.session_id}));
        }

        info!(turn, session_id = %ctx.session_id, "agentic turn");

        match run_turn(&ctx, &tool_defs, &mut messages).await {
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
    tool_defs: &[ToolDef],
    messages: &mut Vec<ChatMessage>,
) -> Result<TurnResult, ForgeError> {
    let assistant_msg =
        session::insert_message(&ctx.pool, ctx.session_id, "assistant").await?;
    (ctx.emit)("forge://message-updated", json!({"info": assistant_msg}));

    let mut event_stream = ctx.provider.stream(&ctx.system_prompt, messages, tool_defs).await?;

    let mut text_buf = String::new();
    let mut reasoning_buf = String::new();
    let text_part_id = Uuid::new_v4();
    let reasoning_part_id = Uuid::new_v4();
    let mut tool_calls: HashMap<String, (String, String)> = HashMap::new();
    let mut stop_reason = StopReason::EndTurn;
    let mut assistant_content: Vec<ContentBlock> = Vec::new();
    let mut tool_results: Vec<ContentBlock> = Vec::new();

    while let Some(event) = event_stream.next().await {
        let event = event?;
        match event {
            StreamEvent::TextDelta(t) => {
                text_buf.push_str(&t);
                let part = session::upsert_part(
                    &ctx.pool, text_part_id, assistant_msg.id, "text", &text_buf, None, None, None,
                ).await?;
                (ctx.emit)("forge://part-updated", json!({"part": part}));
            }
            StreamEvent::ReasoningDelta(t) => {
                reasoning_buf.push_str(&t);
                let part = session::upsert_part(
                    &ctx.pool, reasoning_part_id, assistant_msg.id, "reasoning", &reasoning_buf, None, None, None,
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

                    assistant_content.push(ContentBlock::ToolUse {
                        id: id.clone(), name: name.clone(), input: args.clone(),
                    });

                    let tool_part_id = Uuid::new_v4();
                    let title = format_tool_title(&name, &args);
                    let part = session::upsert_part(
                        &ctx.pool, tool_part_id, assistant_msg.id, "tool", "", Some(&name),
                        Some(&json!({"status": "running", "title": title})),
                        Some(&args),
                    ).await?;
                    (ctx.emit)("forge://part-updated", json!({"part": part}));

                    let result = execute_with_permission(ctx, &name, &args).await?;
                    let is_error = result.is_err();
                    let content = result.unwrap_or_else(|e| e);
                    let status = if is_error { "error" } else { "completed" };

                    let part = session::upsert_part(
                        &ctx.pool, tool_part_id, assistant_msg.id, "tool", &content, Some(&name),
                        Some(&json!({"status": status, "title": title})),
                        Some(&args),
                    ).await?;
                    (ctx.emit)("forge://part-updated", json!({"part": part}));

                    tool_results.push(ContentBlock::ToolResult {
                        tool_use_id: id, content, is_error,
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

    if !text_buf.is_empty() {
        assistant_content.insert(0, ContentBlock::Text { text: text_buf });
    }

    messages.push(ChatMessage { role: Role::Assistant, content: assistant_content });
    if !tool_results.is_empty() {
        messages.push(ChatMessage { role: Role::User, content: tool_results });
    }

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
    if tool_name == "question" {
        return execute_question(ctx, args).await;
    }

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

async fn execute_question(
    ctx: &LoopContext,
    args: &Value,
) -> Result<Result<String, String>, ForgeError> {
    let questions: Vec<QuestionInfo> = serde_json::from_value(
        args.get("questions").cloned().unwrap_or(json!([])),
    )
    .map_err(|e| ForgeError::Other(format!("invalid question args: {e}")))?;

    if questions.is_empty() {
        return Ok(Err("questions array must not be empty".into()));
    }

    let (req, rx) = ctx.questions.ask(ctx.session_id, questions).await;
    (ctx.emit)("forge://question-asked", serde_json::to_value(&req).unwrap());

    let (event, result) = match rx.await.unwrap_or(QuestionResponse::Rejected) {
        QuestionResponse::Answered(a) => (
            "forge://question-replied",
            Ok(serde_json::to_string(&a).unwrap_or_default()),
        ),
        QuestionResponse::Rejected => (
            "forge://question-rejected",
            Err("User skipped the question.".into()),
        ),
    };
    (ctx.emit)(event, json!({"requestID": req.id}));
    Ok(result)
}

async fn build_history(
    pool: &PgPool,
    session_id: Uuid,
) -> Result<(Vec<ChatMessage>, Option<String>), ForgeError> {
    let session = session::get_session(pool, session_id).await?;

    let (rows, summary_text) = if let Some(summary_id) = session.summary_message_id {
        let summary_parts = session::get_parts_for_message(pool, summary_id).await?;
        let text: String = summary_parts.iter()
            .filter(|p| p.part_type == "text")
            .map(|p| p.content.as_str())
            .collect::<Vec<_>>()
            .join("");
        let rows = session::get_messages_after(pool, session_id, summary_id).await?;
        (rows, if text.is_empty() { None } else { Some(text) })
    } else {
        (session::get_messages_with_parts(pool, session_id).await?, None)
    };

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
                                let input = p.tool_input.clone().unwrap_or(json!({}));
                                content.push(ContentBlock::ToolUse {
                                    id: id.clone(), name: name.clone(), input,
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
    Ok((messages, summary_text))
}

async fn generate_title(pool: PgPool, session_id: Uuid, user_text: String, config: ForgeConfig, emit: EmitFn) {
    let provider = provider::build_provider(
        &config.provider, &config.model, config.api_key.as_deref(), config.region.as_deref(),
    );
    let messages = [ChatMessage {
        role: Role::User,
        content: vec![ContentBlock::Text { text: user_text }],
    }];
    let Ok(mut stream) = provider.stream(
        "Generate a short title (3-6 words) for this conversation. Reply with ONLY the title, nothing else.",
        &messages, &[],
    ).await else { return };

    let mut title = String::new();
    while let Some(event) = stream.next().await {
        if let Ok(StreamEvent::TextDelta(t)) = event { title.push_str(&t) }
    }

    let title = title.trim();
    if title.is_empty() { return }
    if session::update_title(&pool, session_id, title).await.is_err() { return }
    if let Ok(s) = session::get_session(&pool, session_id).await {
        (emit)("forge://session-updated", json!({ "session": s }));
    }
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
        "web_fetch" => format!("Fetching {}", args["url"].as_str().unwrap_or("URL")),
        "question" => "Asking user...".to_string(),
        _ => name.to_string(),
    }
}
