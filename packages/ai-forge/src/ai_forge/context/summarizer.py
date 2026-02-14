"""Token-based context management and conversation summarization."""

from __future__ import annotations

__all__ = [
    "CONTEXT_LIMIT",
    "SAFE_THRESHOLD",
    "estimate_tokens",
    "needs_summarization",
    "supersede_writes",
    "prune_tool_outputs",
    "summarize_until_fits",
]

import json
import logging

try:
    import tiktoken
    _ENCODING = tiktoken.get_encoding("cl100k_base")
except ImportError:
    _ENCODING = None

from langchain_aws import ChatBedrockConverse
from langchain_core.messages import (
    AIMessage,
    AnyMessage,
    HumanMessage,
    SystemMessage,
    ToolMessage,
)

from ai_forge.config import ForgeConfig

logger = logging.getLogger(__name__)

CONTEXT_LIMIT = 200_000
SAFE_THRESHOLD = 0.80  # Summarize at 80% capacity
PROTECTED_TOKENS = 40_000  # Keep recent messages
TARGET_AFTER_SUMMARY = 0.50  # Aim for 50% after summarization


def _count_tokens(text: str) -> int:
    """Count tokens using tiktoken if available, otherwise fall back to char/4."""
    if _ENCODING is not None:
        return len(_ENCODING.encode(text, disallowed_special=()))
    return len(text) // 4


def estimate_tokens(messages: list[AnyMessage]) -> int:
    """Estimate token count for a list of messages.

    Uses tiktoken's cl100k_base encoding (close approximation for Claude,
    overestimates by ~5% which is conservative/safe). Falls back to char/4
    if tiktoken is unavailable.
    """
    total = 0
    for msg in messages:
        if isinstance(msg.content, str):
            total += _count_tokens(msg.content)
        elif isinstance(msg.content, list):
            for block in msg.content:
                if isinstance(block, dict):
                    total += _count_tokens(json.dumps(block))
                elif isinstance(block, str):
                    total += _count_tokens(block)
        # Count tool calls
        if hasattr(msg, "tool_calls") and msg.tool_calls:
            total += _count_tokens(json.dumps([tc for tc in msg.tool_calls]))
    return total


def needs_summarization(messages: list[AnyMessage]) -> bool:
    """Check if messages exceed the safe threshold."""
    tokens = estimate_tokens(messages)
    threshold = int(CONTEXT_LIMIT * SAFE_THRESHOLD)
    return tokens > threshold


def supersede_writes(messages: list[AnyMessage]) -> list[AnyMessage]:
    """Replace content of superseded write_file tool calls with a short stub.

    Instead of removing entire AI+Tool message pairs (which breaks tool_call_id
    chains), we replace the *content argument* of superseded write_file calls and
    their corresponding ToolMessage content.  A write is superseded if a later
    write_file targets the same path, or if a read_file for the same path appears
    after the write (meaning the agent re-read the file, so the old write content
    is redundant).
    """
    # Pass 1: find all write_file calls with their (message_index, tc_index, path)
    writes: list[tuple[int, int, str]] = []  # (msg_idx, tc_idx, path)
    for i, msg in enumerate(messages):
        if isinstance(msg, AIMessage) and msg.tool_calls:
            for j, tc in enumerate(msg.tool_calls):
                if tc.get("name") == "write_file":
                    path = tc.get("args", {}).get("path", "")
                    if path:
                        writes.append((i, j, path))

    if not writes:
        return messages

    # Pass 2: find last write index and any read_file indices per path
    last_write: dict[str, int] = {}  # path -> msg_idx of last write
    read_after: dict[str, int] = {}  # path -> msg_idx of last read_file

    for msg_idx, _tc_idx, path in writes:
        last_write[path] = msg_idx

    for i, msg in enumerate(messages):
        if isinstance(msg, AIMessage) and msg.tool_calls:
            for tc in msg.tool_calls:
                if tc.get("name") == "read_file":
                    path = tc.get("args", {}).get("path", "")
                    if path:
                        read_after[path] = i

    # Pass 3: determine which (msg_idx, tc_idx) are superseded
    superseded_tcs: set[tuple[int, int]] = set()  # (msg_idx, tc_idx)
    superseded_tool_ids: set[str] = set()

    for msg_idx, tc_idx, path in writes:
        is_superseded = False
        # Superseded if a later write exists for same path
        if last_write.get(path, -1) > msg_idx:
            is_superseded = True
        # Superseded if a read_file for this path comes after this write
        if read_after.get(path, -1) > msg_idx:
            is_superseded = True

        if is_superseded:
            superseded_tcs.add((msg_idx, tc_idx))
            tc = messages[msg_idx].tool_calls[tc_idx]  # type: ignore[union-attr]
            tool_id = tc.get("id", "")
            if tool_id:
                superseded_tool_ids.add(tool_id)

    if not superseded_tcs:
        return messages

    # Pass 4: build new message list with stubbed content
    result: list[AnyMessage] = []
    for i, msg in enumerate(messages):
        if isinstance(msg, AIMessage) and msg.tool_calls:
            # Check if any tool calls in this message need stubbing
            needs_stub = any((i, j) in superseded_tcs for j in range(len(msg.tool_calls)))
            if needs_stub:
                new_tcs = []
                for j, tc in enumerate(msg.tool_calls):
                    if (i, j) in superseded_tcs:
                        # Count lines in the original content
                        original_content = tc.get("args", {}).get("content", "")
                        line_count = original_content.count("\n") + 1
                        stubbed_tc = {
                            **tc,
                            "args": {
                                **tc.get("args", {}),
                                "content": f"[{line_count} lines — superseded by later write]",
                            },
                        }
                        new_tcs.append(stubbed_tc)
                    else:
                        new_tcs.append(tc)
                # Create new AIMessage with modified tool calls
                new_msg = AIMessage(
                    content=msg.content,
                    tool_calls=new_tcs,
                    id=msg.id,
                )
                result.append(new_msg)
                continue

        if isinstance(msg, ToolMessage) and msg.tool_call_id in superseded_tool_ids:
            result.append(
                ToolMessage(
                    content="[superseded]",
                    tool_call_id=msg.tool_call_id,
                    name=msg.name,
                    id=msg.id,
                )
            )
            continue

        result.append(msg)

    logger.info(
        "Superseded %d write_file tool calls (stubbed content, preserved message structure)",
        len(superseded_tcs),
    )
    return result


def prune_tool_outputs(messages: list[AnyMessage]) -> list[AnyMessage]:
    """Truncate large tool outputs outside the protected recent window.

    Only truncates ToolMessages with > 80K chars (~20K tokens) that are
    outside the last PROTECTED_TOKENS window.
    """
    max_tool_chars = 80_000  # ~20K tokens

    # Find the protected window boundary (last PROTECTED_TOKENS worth)
    protected_boundary = len(messages)
    tokens_from_end = 0
    for i in range(len(messages) - 1, -1, -1):
        tokens_from_end += estimate_tokens([messages[i]])
        if tokens_from_end > PROTECTED_TOKENS:
            protected_boundary = i + 1
            break

    result: list[AnyMessage] = []
    for i, msg in enumerate(messages):
        if (
            i < protected_boundary
            and isinstance(msg, ToolMessage)
            and isinstance(msg.content, str)
            and len(msg.content) > max_tool_chars
        ):
            truncated = msg.content[:8000] + "\n\n[... truncated from {0} chars]".format(
                len(msg.content)
            )
            result.append(
                ToolMessage(
                    content=truncated,
                    tool_call_id=msg.tool_call_id,
                    name=msg.name,
                    id=msg.id,
                )
            )
            continue
        result.append(msg)
    return result


async def summarize_until_fits(
    messages: list[AnyMessage],
    existing_summary: str | None,
    config: ForgeConfig,
    *,
    target_ratio: float = TARGET_AFTER_SUMMARY,
) -> list[AnyMessage]:
    """Summarize older messages until we're under the target threshold.

    Returns new message list with a synthetic summary HumanMessage injected
    after the system message.  Uses LangGraph-compatible message replacement.
    """
    target_tokens = int(CONTEXT_LIMIT * target_ratio)

    # First try non-destructive approaches
    messages = prune_tool_outputs(messages)
    messages = supersede_writes(messages)

    if estimate_tokens(messages) <= target_tokens:
        return messages

    # Separate system message
    system_msg = messages[0] if messages and isinstance(messages[0], SystemMessage) else None
    work_messages = messages[1:] if system_msg else messages

    # Count from the end to find what fits in protected space
    protected_count = 0
    protected_tokens = 0
    for msg in reversed(work_messages):
        msg_tokens = estimate_tokens([msg])
        if protected_tokens + msg_tokens > PROTECTED_TOKENS:
            break
        protected_tokens += msg_tokens
        protected_count += 1

    if protected_count == 0:
        protected_count = 1  # Keep at least the last message

    to_summarize = work_messages[:-protected_count]
    to_keep = work_messages[-protected_count:]

    if not to_summarize:
        return messages

    # Build summary prompt
    summary_text = _format_messages_for_summary(to_summarize)

    context = ""
    if existing_summary:
        context = f"Previous summary:\n{existing_summary}\n\n"

    llm = ChatBedrockConverse(
        model=config.summarizer_model_id,
        region_name=config.aws_region,
        max_tokens=4096,
        temperature=0,
    )

    summary_prompt = (
        f"{context}"
        f"Summarize this conversation segment concisely. "
        f"Focus on: what was built, key decisions, current state of the project, "
        f"and any unresolved issues.\n\n{summary_text}"
    )

    try:
        response = await llm.ainvoke([HumanMessage(content=summary_prompt)])
        new_summary = response.content if isinstance(response.content, str) else str(response.content)
    except Exception as exc:
        logger.warning("Summarization failed: %s", exc)
        new_summary = existing_summary or "Previous conversation context was too large to summarize."

    # Build synthetic summary message
    summary_msg = HumanMessage(
        content=f"[Conversation summary — the following is a summary of our earlier conversation]\n\n{new_summary}",
        additional_kwargs={"is_summary": True},
    )

    # Rebuild: system + summary + (optional ack) + kept messages
    result: list[AnyMessage] = []
    if system_msg:
        result.append(system_msg)
    result.append(summary_msg)

    # Handle role alternation: if first kept message is also HumanMessage,
    # insert a synthetic AI ack to maintain alternation
    if to_keep and isinstance(to_keep[0], HumanMessage):
        result.append(AIMessage(content="Understood. Continuing from where we left off."))

    # If first kept message is a ToolMessage, we need the preceding AIMessage with tool_calls
    # Walk backwards to find it and include it
    if to_keep and isinstance(to_keep[0], ToolMessage):
        result.append(AIMessage(content="Understood. Continuing from where we left off."))

    result.extend(to_keep)

    logger.info(
        "Summarized %d messages, kept %d, new total tokens: ~%d",
        len(to_summarize),
        len(to_keep),
        estimate_tokens(result),
    )

    return result


def _format_messages_for_summary(messages: list[AnyMessage]) -> str:
    """Format messages into a readable string for the summarizer."""
    parts = []
    for msg in messages:
        role = msg.__class__.__name__.replace("Message", "")
        if isinstance(msg.content, str):
            content = msg.content[:2000]
        elif isinstance(msg.content, list):
            content = json.dumps(msg.content, default=str)[:2000]
        else:
            content = str(msg.content)[:2000]
        parts.append(f"[{role}]: {content}")
    return "\n".join(parts)
