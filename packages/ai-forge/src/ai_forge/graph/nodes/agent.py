"""Agent node — invokes Claude via Bedrock with streaming."""

from __future__ import annotations

import logging
from typing import Any

from langchain_aws import ChatBedrockConverse
from langchain_core.messages import AIMessage
from langchain_core.runnables import RunnableConfig

from ai_forge.config import ForgeConfig
from ai_forge.context.cancellation import ForgeCancellation
from ai_forge.context.manager import ContextManager
from ai_forge.graph.state import BuildState
from ai_forge.server.sse import ForgeBroadcaster
from ai_forge.tools.definitions import TOOL_DEFINITIONS

logger = logging.getLogger(__name__)

# Error patterns that indicate context length overflow
_CONTEXT_OVERFLOW_PATTERNS = (
    "too long",
    "too many tokens",
    "context length",
    "maximum context",
    "token limit",
    "input is too long",
)


def _parse_stream_chunk(event: dict) -> str | None:
    """Extract text from an ``on_chat_model_stream`` event, or return *None*."""
    chunk = event.get("data", {}).get("chunk")
    if chunk is None or not hasattr(chunk, "content"):
        return None

    content = chunk.content
    if isinstance(content, str) and content:
        return content

    if isinstance(content, list):
        parts: list[str] = []
        for block in content:
            if isinstance(block, dict) and block.get("type") == "text" and block.get("text"):
                parts.append(block["text"])
        return "".join(parts) or None

    return None


async def _invoke_llm(
    messages: list,
    forge_config: ForgeConfig,
    broadcaster: ForgeBroadcaster,
    project_id: str,
    cancellation: ForgeCancellation,
) -> AIMessage | None:
    """Stream an LLM call and return the final AIMessage, or None if cancelled/empty."""
    llm = ChatBedrockConverse(
        model=forge_config.model_id,
        region_name=forge_config.aws_region,
        max_tokens=forge_config.llm_max_tokens,
        temperature=forge_config.llm_temperature,
    )
    llm_with_tools = llm.bind_tools(TOOL_DEFINITIONS)

    full_response: AIMessage | None = None

    async for event in llm_with_tools.astream_events(
        messages,
        version="v2",
    ):
        if cancellation.is_cancelled(project_id):
            return None

        kind = event.get("event", "")

        if kind == "on_chat_model_stream":
            text = _parse_stream_chunk(event)
            if text:
                await broadcaster.broadcast(project_id, {
                    "type": "agent_thinking",
                    "content": text,
                })

        elif kind == "on_chat_model_end":
            output = event.get("data", {}).get("output")
            if isinstance(output, AIMessage):
                full_response = output

    return full_response


def _is_context_overflow(exc: Exception) -> bool:
    """Check if an exception is a context length overflow error."""
    msg = str(exc).lower()
    return any(pattern in msg for pattern in _CONTEXT_OVERFLOW_PATTERNS)


async def agent_node(
    state: BuildState,
    config: RunnableConfig,
) -> dict[str, Any]:
    """Call Claude via Bedrock, stream thinking to SSE, return AI message."""
    forge_config: ForgeConfig = config["configurable"]["forge_config"]
    broadcaster: ForgeBroadcaster = config["configurable"]["broadcaster"]
    cancellation: ForgeCancellation = config["configurable"]["cancellation"]
    project_id = state["project_id"]

    # Check cancellation
    if cancellation.is_cancelled(project_id):
        return {"phase": "stopped"}

    # Prepare messages with context management (layers 1-3)
    ctx = ContextManager(forge_config)
    messages = list(state["messages"])

    messages = await ctx.prepare_messages(messages)

    # Broadcast phase
    await broadcaster.broadcast(project_id, {
        "type": "phase",
        "phase": state.get("phase", "analyzing"),
    })

    try:
        full_response = await _invoke_llm(
            messages, forge_config, broadcaster, project_id, cancellation,
        )
    except Exception as exc:
        if _is_context_overflow(exc):
            # Layer 4: aggressive reduction + retry
            logger.warning("Context overflow, applying aggressive reduction and retrying...")
            await broadcaster.broadcast(project_id, {
                "type": "status",
                "message": "Context too large, compressing and retrying...",
            })
            messages = await ctx.handle_overflow(messages)
            try:
                full_response = await _invoke_llm(
                    messages, forge_config, broadcaster, project_id, cancellation,
                )
            except Exception as retry_exc:
                logger.exception("Agent LLM call failed after overflow retry")
                return {
                    "error": str(retry_exc),
                    "phase": "error",
                }
        else:
            logger.exception("Agent LLM call failed")
            return {
                "error": str(exc),
                "phase": "error",
            }

    if full_response is None:
        if cancellation.is_cancelled(project_id):
            return {"phase": "stopped"}
        return {
            "error": "No response from LLM",
            "phase": "error",
        }

    # Broadcast tool calls if any
    if full_response.tool_calls:
        await broadcaster.broadcast(project_id, {
            "type": "tool_calls",
            "calls": [
                {"name": tc["name"], "args": tc.get("args", {})}
                for tc in full_response.tool_calls
            ],
        })

    # Determine phase based on response
    phase = state.get("phase", "analyzing")
    if full_response.tool_calls:
        phase = "executing"
    else:
        phase = "done"

    return {
        "messages": [full_response],
        "phase": phase,
        "iteration": state.get("iteration", 0) + 1,
    }
