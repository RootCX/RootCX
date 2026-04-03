use bytes::Bytes;
use futures::StreamExt;
use serde_json::{json, Value};

use super::{
    ChatMessage, ContentBlock, EventStream, LlmProvider, Role, StopReason, StreamEvent, ToolDef,
};
use crate::error::ForgeError;

const DEFAULT_BASE_URL: &str = "https://api.openai.com";
const MAX_TOKENS: u32 = 16384;

pub struct OpenAi {
    model: String,
    api_key: String,
    base_url: String,
    client: reqwest::Client,
}

impl OpenAi {
    pub fn new(model: String, api_key: String) -> Self {
        Self {
            model,
            api_key,
            base_url: DEFAULT_BASE_URL.into(),
            client: reqwest::Client::new(),
        }
    }

    pub fn with_base_url(model: String, api_key: String, base_url: String) -> Self {
        Self {
            model,
            api_key,
            base_url,
            client: reqwest::Client::new(),
        }
    }

    fn build_messages(system: &str, messages: &[ChatMessage]) -> Vec<Value> {
        let mut out = vec![json!({"role": "system", "content": system})];

        for m in messages {
            let role = match m.role {
                Role::User => "user",
                Role::Assistant => "assistant",
            };

            // Simple text-only message
            if m.content.len() == 1 {
                if let ContentBlock::Text { text } = &m.content[0] {
                    out.push(json!({"role": role, "content": text}));
                    continue;
                }
            }

            // Assistant with tool calls
            if matches!(m.role, Role::Assistant)
                && m.content.iter().any(|b| matches!(b, ContentBlock::ToolUse { .. }))
            {
                let text = m
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");

                let tool_calls: Vec<Value> = m
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::ToolUse { id, name, input } => Some(json!({
                            "id": id,
                            "type": "function",
                            "function": { "name": name, "arguments": input.to_string() }
                        })),
                        _ => None,
                    })
                    .collect();

                let mut msg = json!({"role": "assistant", "tool_calls": tool_calls});
                if !text.is_empty() {
                    msg["content"] = json!(text);
                }
                out.push(msg);
                continue;
            }

            // Tool results
            for block in &m.content {
                if let ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    ..
                } = block
                {
                    out.push(json!({
                        "role": "tool",
                        "tool_call_id": tool_use_id,
                        "content": content,
                    }));
                }
            }
        }

        out
    }

    fn build_tools(tools: &[ToolDef]) -> Vec<Value> {
        tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.input_schema,
                    }
                })
            })
            .collect()
    }
}

#[async_trait::async_trait]
impl LlmProvider for OpenAi {
    fn context_window(&self) -> usize {
        if self.model.contains("gpt-4.1") { 1_000_000 }
        else if self.model.contains("gpt-4o") || self.model.contains("gpt-4-turbo") { 128_000 }
        else if self.model.starts_with("o1") || self.model.starts_with("o3") { 200_000 }
        else { 128_000 }
    }

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
            "messages": Self::build_messages(system, messages),
        });
        if !tools.is_empty() {
            body["tools"] = json!(Self::build_tools(tools));
        }

        let resp = self
            .client
            .post(format!("{}/v1/chat/completions", self.base_url))
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            tracing::error!(status = %status, "openai API error: {body}");
            let msg = match status.as_u16() {
                401 => "authentication failed — check your API key",
                429 => "rate limited — try again shortly",
                500..=599 => "provider server error — try again later",
                _ => "provider request failed",
            };
            return Err(ForgeError::Provider(format!("{status}: {msg}")));
        }

        let byte_stream = resp.bytes_stream();
        type ByteStream = std::pin::Pin<
            Box<dyn futures::Stream<Item = Result<Bytes, reqwest::Error>> + Send>,
        >;
        let bs: ByteStream = Box::pin(byte_stream);

        let stream = futures::stream::unfold(
            (bs, String::new(), StopState::default()),
            |(mut bs, mut buf, mut stop_state): (ByteStream, String, StopState)| async move {
                loop {
                    if let Some(pos) = buf.find('\n') {
                        let line = buf[..pos].trim_end_matches('\r').to_string();
                        buf.drain(..pos + 1);

                        if line.is_empty() {
                            continue;
                        }
                        if let Some(data) = line.strip_prefix("data: ") {
                            if data == "[DONE]" {
                                let reason = stop_state
                                    .reason
                                    .take()
                                    .unwrap_or(StopReason::EndTurn);
                                return Some((Ok(StreamEvent::Done(reason)), (bs, buf, stop_state)));
                            }
                            if let Some(evt) = parse_chunk(data, &mut stop_state) {
                                return Some((evt, (bs, buf, stop_state)));
                            }
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

#[derive(Default)]
struct StopState {
    reason: Option<StopReason>,
}

fn parse_chunk(
    data: &str,
    stop_state: &mut StopState,
) -> Option<Result<StreamEvent, ForgeError>> {
    let json: Value = serde_json::from_str(data).ok()?;
    let choice = json["choices"].get(0)?;

    // Capture stop reason
    if let Some(reason) = choice["finish_reason"].as_str() {
        stop_state.reason = Some(match reason {
            "stop" => StopReason::EndTurn,
            "tool_calls" => StopReason::ToolUse,
            "length" => StopReason::MaxTokens,
            _ => StopReason::EndTurn,
        });
    }

    let delta = &choice["delta"];

    // Text content
    if let Some(text) = delta["content"].as_str() {
        if !text.is_empty() {
            return Some(Ok(StreamEvent::TextDelta(text.to_string())));
        }
    }

    // Tool calls
    if let Some(tool_calls) = delta["tool_calls"].as_array() {
        for tc in tool_calls {
            let idx = tc["index"].as_u64().unwrap_or(0).to_string();
            if let Some(func) = tc.get("function") {
                if let Some(name) = func["name"].as_str() {
                    return Some(Ok(StreamEvent::ToolCallStart {
                        id: tc["id"].as_str().unwrap_or(&idx).to_string(),
                        name: name.to_string(),
                    }));
                }
                if let Some(args) = func["arguments"].as_str() {
                    if !args.is_empty() {
                        return Some(Ok(StreamEvent::ToolCallDelta {
                            id: tc["id"]
                                .as_str()
                                .unwrap_or(&idx)
                                .to_string(),
                            json: args.to_string(),
                        }));
                    }
                }
            }
        }
    }

    None
}
