"""Graph state types and reducers."""

from __future__ import annotations

__all__ = ["BuildState", "FileChange", "PlanStep", "CodeError", "messages_reducer"]

from dataclasses import dataclass
from typing import Annotated, Literal, TypedDict

from langchain_core.messages import AIMessage, AnyMessage, ToolMessage
from langgraph.graph.message import add_messages


# ── Data models ─────────────────────────────────────────────────────


@dataclass
class PlanStep:
    description: str
    status: Literal["pending", "in_progress", "done", "skipped"] = "pending"


@dataclass
class FileChange:
    path: str
    action: Literal["create", "update", "delete"]


@dataclass
class CodeError:
    file: str
    line: int | None
    message: str


# ── Custom messages reducer ─────────────────────────────────────────

_MAX_MESSAGES = 200


def messages_reducer(
    existing: list[AnyMessage],
    update: list[AnyMessage] | AnyMessage,
) -> list[AnyMessage]:
    """Append messages via LangGraph's add_messages, enforce hard cap.

    Protects against orphaned ToolMessages: when trimming, ensure we
    don't start the kept window with a ToolMessage that has no matching
    AIMessage with the corresponding tool_call.
    """
    result = add_messages(existing, update)
    if len(result) <= _MAX_MESSAGES:
        return list(result)
    if not result:
        return list(result)

    # Keep system message (first) + most recent messages
    system = result[0]
    tail = list(result[-(_MAX_MESSAGES - 1):])

    # Walk forward to find a safe start: skip orphaned ToolMessages
    while tail and isinstance(tail[0], ToolMessage):
        # Check if a preceding AIMessage with this tool_call_id exists in tail
        orphaned = True
        tool_call_id = tail[0].tool_call_id
        for msg in tail[1:]:
            if isinstance(msg, AIMessage) and msg.tool_calls:
                if any(tc.get("id") == tool_call_id for tc in msg.tool_calls):
                    orphaned = False
                    break
        if orphaned:
            tail = tail[1:]
        else:
            break

    return [system] + tail


# ── Graph state ─────────────────────────────────────────────────────

class BuildState(TypedDict):
    """Full state flowing through the LangGraph build workflow."""

    project_id: str
    project_path: str
    user_prompt: str
    conversation_id: str
    app_id: str

    messages: Annotated[list[AnyMessage], messages_reducer]
    conversation_summary: str | None

    phase: Literal[
        "analyzing", "planning", "executing",
        "verifying", "done", "error", "stopped",
    ]
    thinking: str
    plan: list[PlanStep]
    applied_changes: list[FileChange]
    errors: list[CodeError]

    iteration: int
    max_iterations: int
    success: bool
    message: str
    error: str | None
