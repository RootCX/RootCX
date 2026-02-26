use std::collections::HashMap;

use aws_sdk_bedrockruntime as bedrock;
use aws_smithy_types::Document;

use super::{
    ChatMessage, ContentBlock, EventStream, LlmProvider, Role, StopReason, StreamEvent, ToolDef,
};
use crate::error::ForgeError;

pub struct Bedrock {
    model: String,
    region: String,
    bearer_token: Option<String>,
}

impl Bedrock {
    pub fn new(model: String, region: Option<String>, bearer_token: Option<String>) -> Self {
        Self {
            model,
            region: region.unwrap_or_else(|| "us-east-1".into()),
            bearer_token,
        }
    }

    fn convert_messages(
        messages: &[ChatMessage],
    ) -> Result<Vec<bedrock::types::Message>, ForgeError> {
        messages
            .iter()
            .map(|m| {
                let role = match m.role {
                    Role::User => bedrock::types::ConversationRole::User,
                    Role::Assistant => bedrock::types::ConversationRole::Assistant,
                };
                let blocks: Vec<bedrock::types::ContentBlock> = m
                    .content
                    .iter()
                    .filter_map(|b| Self::convert_content_block(b))
                    .collect();

                bedrock::types::Message::builder()
                    .role(role)
                    .set_content(Some(blocks))
                    .build()
                    .map_err(|e| ForgeError::Provider(e.to_string()))
            })
            .collect()
    }

    fn convert_content_block(b: &ContentBlock) -> Option<bedrock::types::ContentBlock> {
        match b {
            ContentBlock::Text { text } => {
                Some(bedrock::types::ContentBlock::Text(text.clone()))
            }
            ContentBlock::ToolUse { id, name, input } => {
                let doc = json_to_document(input);
                let tu = bedrock::types::ToolUseBlock::builder()
                    .tool_use_id(id)
                    .name(name)
                    .input(doc)
                    .build()
                    .ok()?;
                Some(bedrock::types::ContentBlock::ToolUse(tu))
            }
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                let status = if *is_error {
                    bedrock::types::ToolResultStatus::Error
                } else {
                    bedrock::types::ToolResultStatus::Success
                };
                let tr = bedrock::types::ToolResultBlock::builder()
                    .tool_use_id(tool_use_id)
                    .content(bedrock::types::ToolResultContentBlock::Text(content.clone()))
                    .status(status)
                    .build()
                    .ok()?;
                Some(bedrock::types::ContentBlock::ToolResult(tr))
            }
        }
    }

    fn convert_tools(tools: &[ToolDef]) -> Option<bedrock::types::ToolConfiguration> {
        if tools.is_empty() {
            return None;
        }
        let specs: Vec<bedrock::types::Tool> = tools
            .iter()
            .filter_map(|t| {
                let doc = json_to_document(&t.input_schema);
                let spec = bedrock::types::ToolSpecification::builder()
                    .name(&t.name)
                    .description(&t.description)
                    .input_schema(bedrock::types::ToolInputSchema::Json(doc))
                    .build()
                    .ok()?;
                Some(bedrock::types::Tool::ToolSpec(spec))
            })
            .collect();
        bedrock::types::ToolConfiguration::builder()
            .set_tools(Some(specs))
            .build()
            .ok()
    }
}

#[async_trait::async_trait]
impl LlmProvider for Bedrock {
    fn context_window(&self) -> usize {
        if self.model.contains("claude") { 200_000 } else { 128_000 }
    }

    async fn stream(
        &self,
        system: &str,
        messages: &[ChatMessage],
        tools: &[ToolDef],
    ) -> Result<EventStream, ForgeError> {
        let config_loader = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(aws_config::Region::new(self.region.clone()));

        // Bearer token: inject as env var for AWS SDK credential chain
        if let Some(ref token) = self.bearer_token {
            // SAFETY: called before AWS SDK reads env, single call site
            unsafe { std::env::set_var("AWS_BEARER_TOKEN_BEDROCK", token); }
        }

        let config = config_loader.load().await;
        let client = bedrock::Client::new(&config);

        let br_messages = Self::convert_messages(messages)?;

        let mut req = client
            .converse_stream()
            .model_id(&self.model)
            .system(bedrock::types::SystemContentBlock::Text(system.to_string()))
            .set_messages(Some(br_messages));

        if let Some(tc) = Self::convert_tools(tools) {
            req = req.tool_config(tc);
        }

        let output = req
            .send()
            .await
            .map_err(|e| {
                let detail = e.as_service_error()
                    .map(|se| format!("{se:?}"))
                    .unwrap_or_else(|| format!("{e:#}"));
                tracing::error!(model = %self.model, region = %self.region, "bedrock: {detail}");
                ForgeError::Provider(detail)
            })?;

        let stream_handle = output.stream;

        // current_tool_id: correlates ContentBlockDelta/Stop (index-based) to the tool_use_id from Start
        let stream = futures::stream::unfold(
            (stream_handle, None::<String>),
            |(mut s, mut tool_id)| async move {
                loop {
                    match s.recv().await {
                        Ok(Some(event)) => match convert_event(event, &mut tool_id) {
                            Some(evt) => return Some((evt, (s, tool_id))),
                            None => continue,
                        },
                        Ok(None) => return None,
                        Err(e) => return Some((Err(ForgeError::Provider(e.to_string())), (s, tool_id))),
                    }
                }
            },
        );

        Ok(Box::pin(stream))
    }
}

fn convert_event(
    event: bedrock::types::ConverseStreamOutput,
    current_tool_id: &mut Option<String>,
) -> Option<Result<StreamEvent, ForgeError>> {
    use bedrock::types::{ContentBlockDelta as D, ContentBlockStart as S, ConverseStreamOutput as E};

    Some(Ok(match event {
        E::ContentBlockStart(b) => match b.start {
            Some(S::ToolUse(tu)) => {
                *current_tool_id = Some(tu.tool_use_id.clone());
                StreamEvent::ToolCallStart { id: tu.tool_use_id, name: tu.name }
            }
            _ => return None,
        },
        E::ContentBlockDelta(b) => match b.delta {
            Some(D::Text(t)) => StreamEvent::TextDelta(t),
            Some(D::ToolUse(tu)) => StreamEvent::ToolCallDelta {
                id: current_tool_id.clone().unwrap_or_default(),
                json: tu.input,
            },
            _ => return None,
        },
        E::ContentBlockStop(_) => match current_tool_id.take() {
            Some(id) => StreamEvent::ToolCallEnd { id },
            None => return None, // text block stop
        },
        E::MessageStop(s) => StreamEvent::Done(match s.stop_reason {
            bedrock::types::StopReason::ToolUse => StopReason::ToolUse,
            bedrock::types::StopReason::MaxTokens => StopReason::MaxTokens,
            _ => StopReason::EndTurn,
        }),
        _ => return None,
    }))
}

fn json_to_document(value: &serde_json::Value) -> Document {
    match value {
        serde_json::Value::Null => Document::Null,
        serde_json::Value::Bool(b) => Document::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Document::Number(aws_smithy_types::Number::PosInt(i as u64))
            } else if let Some(f) = n.as_f64() {
                Document::Number(aws_smithy_types::Number::Float(f))
            } else {
                Document::Null
            }
        }
        serde_json::Value::String(s) => Document::String(s.clone()),
        serde_json::Value::Array(arr) => {
            Document::Array(arr.iter().map(json_to_document).collect())
        }
        serde_json::Value::Object(map) => {
            let hm: HashMap<String, Document> = map
                .iter()
                .map(|(k, v)| (k.clone(), json_to_document(v)))
                .collect();
            Document::Object(hm)
        }
    }
}
