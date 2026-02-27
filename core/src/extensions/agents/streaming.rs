use std::convert::Infallible;
use std::sync::Arc;

use axum::response::sse::Event;
use futures::stream::Stream;
use serde_json::json;
use tracing::error;

use super::persistence::{self, PersistCtx};
use crate::worker::AgentEvent;

pub(crate) fn build_sse_stream(
    stream_rx: tokio::sync::mpsc::Receiver<AgentEvent>,
    session_id: Arc<str>,
    persist_ctx: Option<PersistCtx>,
) -> impl Stream<Item = Result<Event, Infallible>> {
    futures::stream::unfold(
        (stream_rx, persist_ctx),
        move |(mut rx, ctx)| {
            let sid = Arc::clone(&session_id);
            async move {
                match rx.recv().await {
                    Some(AgentEvent::Chunk { delta }) => {
                        let event = Event::default().event("chunk")
                            .data(json!({"delta": delta, "session_id": &*sid}).to_string());
                        Some((Ok(event), (rx, ctx)))
                    }
                    Some(AgentEvent::Done { response, tokens }) => {
                        if let Some(ref pctx) = ctx {
                            if let Err(e) = persistence::persist_session(pctx, &response, tokens).await {
                                error!(session_id = %sid, "failed to persist session: {e}");
                            }
                        }
                        let event = Event::default().event("done")
                            .data(json!({"response": response, "session_id": &*sid, "tokens": tokens}).to_string());
                        Some((Ok(event), (rx, None)))
                    }
                    Some(AgentEvent::Error { error }) => {
                        let event = Event::default().event("error")
                            .data(json!({"error": error, "session_id": &*sid}).to_string());
                        Some((Ok(event), (rx, None)))
                    }
                    Some(AgentEvent::ToolCallStarted { call_id, tool_name, input }) => {
                        if let Some(ref pctx) = ctx {
                            let _ = persistence::persist_tool_call_start(&pctx.pool, &pctx.session_id, &call_id, &tool_name, &input).await;
                        }
                        let event = Event::default().event("tool_call_started")
                            .data(json!({"call_id": call_id, "tool_name": tool_name, "input": input, "session_id": &*sid}).to_string());
                        Some((Ok(event), (rx, ctx)))
                    }
                    Some(AgentEvent::ToolCallCompleted { call_id, tool_name, output, error, duration_ms }) => {
                        if let Some(ref pctx) = ctx {
                            let _ = persistence::persist_tool_call_end(&pctx.pool, &call_id, output.as_ref(), error.as_deref(), duration_ms).await;
                        }
                        let event = Event::default().event("tool_call_completed")
                            .data(json!({"call_id": call_id, "tool_name": tool_name, "output": output, "error": error, "duration_ms": duration_ms, "session_id": &*sid}).to_string());
                        Some((Ok(event), (rx, ctx)))
                    }
                    Some(AgentEvent::ApprovalRequired { approval_id, tool_name, args, reason }) => {
                        let event = Event::default().event("approval_required")
                            .data(json!({"approval_id": approval_id, "tool_name": tool_name, "args": args, "reason": reason, "session_id": &*sid}).to_string());
                        Some((Ok(event), (rx, ctx)))
                    }
                    Some(AgentEvent::SessionCompacted { summary }) => {
                        if let Some(ref pctx) = ctx {
                            let _ = persistence::persist_message(&pctx.pool, &pctx.session_id, "system", &summary, None, true).await;
                        }
                        let event = Event::default().event("session_compacted")
                            .data(json!({"summary": summary, "session_id": &*sid}).to_string());
                        Some((Ok(event), (rx, ctx)))
                    }
                    None => None,
                }
            }
        },
    )
}
