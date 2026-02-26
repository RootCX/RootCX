pub mod anthropic;
pub mod bedrock;
pub mod openai;

use std::pin::Pin;

use futures::Stream;
use serde::{Deserialize, Serialize};

use crate::error::ForgeError;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Anthropic,
    OpenAi,
    Bedrock,
}

#[derive(Debug, Clone)]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
}

#[derive(Debug, Clone)]
pub enum StreamEvent {
    TextDelta(String),
    ReasoningDelta(String),
    ToolCallStart { id: String, name: String },
    ToolCallDelta { id: String, json: String },
    ToolCallEnd { id: String },
    Done(StopReason),
    Error(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text { text: String },
    ToolUse { id: String, name: String, input: serde_json::Value },
    ToolResult { tool_use_id: String, content: String, is_error: bool },
}

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

pub type EventStream =
    Pin<Box<dyn Stream<Item = Result<StreamEvent, ForgeError>> + Send>>;

#[async_trait::async_trait]
pub trait LlmProvider: Send + Sync {
    async fn stream(
        &self,
        system: &str,
        messages: &[ChatMessage],
        tools: &[ToolDef],
    ) -> Result<EventStream, ForgeError>;

    fn context_window(&self) -> usize;
}

pub fn estimate_tokens(system: &str, messages: &[ChatMessage]) -> usize {
    let mut chars = system.len();
    for msg in messages {
        chars += 4; // per-message overhead
        for block in &msg.content {
            chars += match block {
                ContentBlock::Text { text } => text.len(),
                ContentBlock::ToolUse { name, input, .. } => name.len() + input.to_string().len(),
                ContentBlock::ToolResult { content, .. } => content.len(),
            };
        }
    }
    chars / 4
}

pub fn build_provider(
    kind: &ProviderKind,
    model: &str,
    api_key: Option<&str>,
    region: Option<&str>,
) -> Box<dyn LlmProvider> {
    match kind {
        ProviderKind::Anthropic => Box::new(anthropic::Anthropic::new(
            model.to_string(),
            api_key.unwrap_or_default().to_string(),
        )),
        ProviderKind::OpenAi => Box::new(openai::OpenAi::new(
            model.to_string(),
            api_key.unwrap_or_default().to_string(),
        )),
        ProviderKind::Bedrock => Box::new(bedrock::Bedrock::new(
            model.to_string(),
            region.map(String::from),
            api_key.map(String::from),
        )),
    }
}
