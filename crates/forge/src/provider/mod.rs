pub mod anthropic;
pub mod bedrock;
pub mod openai;

use std::pin::Pin;

use futures::Stream;

use crate::error::ForgeError;

pub use rootcx_types::{ChatMessage, ContentBlock, ProviderType, Role, ToolDef};

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
        chars += 4;
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
    kind: &ProviderType,
    model: &str,
    api_key: Option<&str>,
    region: Option<&str>,
) -> Box<dyn LlmProvider> {
    match kind {
        ProviderType::Anthropic => Box::new(anthropic::Anthropic::new(
            model.to_string(),
            api_key.unwrap_or_default().to_string(),
        )),
        ProviderType::OpenAI => Box::new(openai::OpenAi::new(
            model.to_string(),
            api_key.unwrap_or_default().to_string(),
        )),
        ProviderType::Bedrock => Box::new(bedrock::Bedrock::new(
            model.to_string(),
            region.map(String::from),
            api_key.map(String::from),
        )),
    }
}
