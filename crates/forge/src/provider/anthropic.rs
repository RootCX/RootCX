use bytes::Bytes;
use futures::StreamExt;
use serde_json::{json, Value};

use super::{
    ChatMessage, ContentBlock, EventStream, LlmProvider, Role, StopReason, StreamEvent, ToolDef,
};
use crate::error::ForgeError;

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2023-06-01";
const MAX_TOKENS: u32 = 16384;

pub struct Anthropic {
    model: String,
    api_key: String,
    client: reqwest::Client,
}

impl Anthropic {
    pub fn new(model: String, api_key: String) -> Self {
        Self {
            model,
            api_key,
            client: reqwest::Client::new(),
        }
    }

    fn build_messages(messages: &[ChatMessage]) -> Vec<Value> {
        messages
            .iter()
            .map(|m| {
                let role = match m.role {
                    Role::User => "user",
                    Role::Assistant => "assistant",
                };
                let content: Vec<Value> = m
                    .content
                    .iter()
                    .map(|b| match b {
                        ContentBlock::Text { text } => json!({"type": "text", "text": text}),
                        ContentBlock::ToolUse { id, name, input } => {
                            json!({"type": "tool_use", "id": id, "name": name, "input": input})
                        }
                        ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                        } => json!({
                            "type": "tool_result",
                            "tool_use_id": tool_use_id,
                            "content": content,
                            "is_error": is_error,
                        }),
                    })
                    .collect();
                json!({"role": role, "content": content})
            })
            .collect()
    }

    fn build_tools(tools: &[ToolDef]) -> Vec<Value> {
        tools
            .iter()
            .map(|t| {
                json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": t.input_schema,
                })
            })
            .collect()
    }
}

#[async_trait::async_trait]
impl LlmProvider for Anthropic {
    async fn stream(
        &self,
        system: &str,
        messages: &[ChatMessage],
        tools: &[ToolDef],
    ) -> Result<EventStream, ForgeError> {
        let mut body = json!({
            "model": self.model,
            "max_tokens": MAX_TOKENS,
            "stream": true,
            "system": system,
            "messages": Self::build_messages(messages),
        });
        if !tools.is_empty() {
            body["tools"] = json!(Self::build_tools(tools));
        }

        let resp = self
            .client
            .post(API_URL)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", API_VERSION)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(ForgeError::Provider(format!("{status}: {text}")));
        }

        let byte_stream = resp.bytes_stream();
        type ByteStream = std::pin::Pin<
            Box<dyn futures::Stream<Item = Result<Bytes, reqwest::Error>> + Send>,
        >;
        let bs: ByteStream = Box::pin(byte_stream);

        let stream = futures::stream::unfold(
            (bs, String::new()),
            |(mut bs, mut buf): (ByteStream, String)| async move {
                loop {
                    if let Some(pos) = buf.find("\n\n") {
                        let event_block = buf[..pos].to_string();
                        buf.drain(..pos + 2);

                        if let Some(evt) = parse_sse_event(&event_block) {
                            return Some((evt, (bs, buf)));
                        }
                        continue;
                    }

                    match bs.next().await {
                        Some(Ok(chunk)) => {
                            buf.push_str(&String::from_utf8_lossy(&chunk));
                        }
                        _ => return None,
                    }
                }
            },
        );

        Ok(Box::pin(stream))
    }
}

fn parse_sse_event(block: &str) -> Option<Result<StreamEvent, ForgeError>> {
    let mut event_type = "";
    let mut data = String::new();

    for line in block.lines() {
        if let Some(val) = line.strip_prefix("event: ") {
            event_type = val.trim();
        } else if let Some(val) = line.strip_prefix("data: ") {
            data.push_str(val);
        }
    }

    if data.is_empty() {
        return None;
    }

    let json: Value = match serde_json::from_str(&data) {
        Ok(v) => v,
        Err(e) => return Some(Err(ForgeError::Stream(e.to_string()))),
    };

    match event_type {
        "content_block_start" => {
            let block = &json["content_block"];
            match block["type"].as_str()? {
                "tool_use" => Some(Ok(StreamEvent::ToolCallStart {
                    id: block["id"].as_str()?.to_string(),
                    name: block["name"].as_str()?.to_string(),
                })),
                "thinking" => {
                    let text = block["thinking"].as_str().unwrap_or("");
                    if text.is_empty() {
                        None
                    } else {
                        Some(Ok(StreamEvent::ReasoningDelta(text.to_string())))
                    }
                }
                _ => None,
            }
        }
        "content_block_delta" => {
            let delta = &json["delta"];
            match delta["type"].as_str()? {
                "text_delta" => {
                    Some(Ok(StreamEvent::TextDelta(delta["text"].as_str()?.to_string())))
                }
                "thinking_delta" => Some(Ok(StreamEvent::ReasoningDelta(
                    delta["thinking"].as_str()?.to_string(),
                ))),
                "input_json_delta" => {
                    let id = json["index"].as_u64().map(|i| i.to_string()).unwrap_or_default();
                    Some(Ok(StreamEvent::ToolCallDelta {
                        id,
                        json: delta["partial_json"].as_str()?.to_string(),
                    }))
                }
                _ => None,
            }
        }
        "content_block_stop" => {
            let idx = json["index"].as_u64().map(|i| i.to_string()).unwrap_or_default();
            Some(Ok(StreamEvent::ToolCallEnd { id: idx }))
        }
        "message_delta" => {
            let reason = match json["delta"]["stop_reason"].as_str()? {
                "end_turn" => StopReason::EndTurn,
                "tool_use" => StopReason::ToolUse,
                "max_tokens" => StopReason::MaxTokens,
                _ => StopReason::EndTurn,
            };
            Some(Ok(StreamEvent::Done(reason)))
        }
        "error" => {
            let msg = json["error"]["message"]
                .as_str()
                .unwrap_or("unknown error");
            Some(Ok(StreamEvent::Error(msg.to_string())))
        }
        _ => None,
    }
}
