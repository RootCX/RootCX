"""Tools node — executes tool calls from the agent response."""

from __future__ import annotations

import json
import logging
from dataclasses import dataclass
from typing import Any

from langchain_core.messages import AIMessage, ToolMessage
from langchain_core.runnables import RunnableConfig

from ai_forge.context.cancellation import ForgeCancellation
from ai_forge.graph.state import BuildState, FileChange
from ai_forge.server.sse import ForgeBroadcaster

logger = logging.getLogger(__name__)


@dataclass
class ToolResult:
    """Result of a tool execution."""

    output: str
    change: FileChange | None = None


def _parse_tool_output(raw: str) -> ToolResult:
    """Parse tool output, extracting FileChange if the tool encoded one."""
    try:
        data = json.loads(raw)
        if isinstance(data, dict) and "change" in data:
            change_data = data["change"]
            change = FileChange(path=change_data["path"], action=change_data["action"])
            return ToolResult(output=data.get("message", raw), change=change)
    except (json.JSONDecodeError, KeyError, TypeError):
        pass
    return ToolResult(output=raw)


async def tools_node(
    state: BuildState,
    config: RunnableConfig,
) -> dict[str, Any]:
    """Execute all pending tool calls and return tool messages."""
    broadcaster: ForgeBroadcaster = config["configurable"]["broadcaster"]
    cancellation: ForgeCancellation = config["configurable"]["cancellation"]
    tools: list = config["configurable"]["tools"]
    project_id = state["project_id"]

    # Build name -> tool lookup
    tool_by_name = {t.name: t for t in tools}

    # Get the last AI message with tool calls
    if not state["messages"]:
        return {}
    last_msg = state["messages"][-1]
    if not isinstance(last_msg, AIMessage) or not last_msg.tool_calls:
        return {}

    tool_messages: list[ToolMessage] = []
    applied_changes = list(state.get("applied_changes", []))

    for tc in last_msg.tool_calls:
        if cancellation.is_cancelled(project_id):
            tool_messages.append(
                ToolMessage(
                    content="Cancelled by user.",
                    tool_call_id=tc["id"],
                    name=tc["name"],
                )
            )
            continue

        tool_name = tc["name"]
        tool_args = tc.get("args", {})

        logger.info("Executing tool: %s", tool_name)

        await broadcaster.broadcast(project_id, {
            "type": "tool_executing",
            "name": tool_name,
            "args": tool_args,
        })

        tool_impl = tool_by_name.get(tool_name)
        if tool_impl is None:
            raw_output = f"Unknown tool: {tool_name}"
        else:
            try:
                raw_output = await tool_impl.ainvoke(tool_args)
            except Exception as exc:
                logger.exception("Tool execution failed: %s", tool_name)
                raw_output = f"Error executing {tool_name}: {exc}"

        result = _parse_tool_output(str(raw_output))

        if result.change is not None:
            applied_changes.append(result.change)

        # Broadcast result
        await broadcaster.broadcast(project_id, {
            "type": "tool_result",
            "name": tool_name,
            "output": result.output[:500],  # Preview only
        })

        tool_messages.append(
            ToolMessage(
                content=result.output,
                tool_call_id=tc["id"],
                name=tool_name,
            )
        )

    return {
        "messages": tool_messages,
        "applied_changes": applied_changes,
    }
