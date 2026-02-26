use futures::StreamExt;

use crate::error::ForgeError;
use crate::provider::{ChatMessage, ContentBlock, LlmProvider, Role, StreamEvent};

#[async_trait::async_trait]
pub trait Compactor: Send + Sync {
    async fn compact(
        &self,
        provider: &dyn LlmProvider,
        messages: &[ChatMessage],
        keep: usize,
    ) -> Result<String, ForgeError>;
}

/// Summarizes older messages via an LLM call.
pub struct LlmSummarizer;

#[async_trait::async_trait]
impl Compactor for LlmSummarizer {
    async fn compact(
        &self,
        provider: &dyn LlmProvider,
        messages: &[ChatMessage],
        keep: usize,
    ) -> Result<String, ForgeError> {
        let to_summarize = &messages[..messages.len().saturating_sub(keep)];
        let input = format_for_summary(to_summarize);

        let request = vec![ChatMessage {
            role: Role::User,
            content: vec![ContentBlock::Text { text: input }],
        }];

        let mut stream = provider
            .stream("You are a summarization assistant. Produce a concise summary.", &request, &[])
            .await?;

        let mut summary = String::new();
        while let Some(event) = stream.next().await {
            match event? {
                StreamEvent::TextDelta(t) => summary.push_str(&t),
                StreamEvent::Done(_) | StreamEvent::Error(_) => break,
                _ => {}
            }
        }

        if summary.is_empty() {
            return Err(ForgeError::Other("compaction produced empty summary".into()));
        }
        Ok(summary)
    }
}

fn format_for_summary(messages: &[ChatMessage]) -> String {
    let mut out = String::from(
        "Summarize this coding conversation. Preserve all file paths, key decisions, \
         current task state, and pending work. Be concise but complete.\n\n",
    );
    for msg in messages {
        let role = match msg.role {
            Role::User => "User",
            Role::Assistant => "Assistant",
        };
        out.push_str(&format!("--- {role} ---\n"));
        for block in &msg.content {
            match block {
                ContentBlock::Text { text } => {
                    out.push_str(text);
                    out.push('\n');
                }
                ContentBlock::ToolUse { name, .. } => {
                    out.push_str(&format!("[Called: {name}]\n"));
                }
                ContentBlock::ToolResult { content, is_error, .. } => {
                    let label = if *is_error { "Error" } else { "Result" };
                    let end = content.floor_char_boundary(500);
                    out.push_str(&format!("[{label}: {}]\n", &content[..end]));
                }
            }
        }
    }
    out
}
