"""Tools node — executes tool calls from the agent response."""

from __future__ import annotations

import logging
from typing import Any

from langchain_core.messages import AIMessage, ToolMessage
from langchain_core.runnables import RunnableConfig

from ai_forge.config import ForgeConfig
from ai_forge.context.cancellation import ForgeCancellation
from ai_forge.graph.state import BuildState, FileChange
from ai_forge.server.sse import ForgeBroadcaster
from ai_forge.tools.executor import execute_tool

logger = logging.getLogger(__name__)


async def tools_node(
    state: BuildState,
    config: RunnableConfig,
) -> dict[str, Any]:
    """Execute all pending tool calls and return tool messages."""
    forge_config: ForgeConfig = config["configurable"]["forge_config"]
    broadcaster: ForgeBroadcaster = config["configurable"]["broadcaster"]
    cancellation: ForgeCancellation = config["configurable"]["cancellation"]
    project_id = state["project_id"]

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

        result = await execute_tool(
            tool_name,
            tool_args,
            config=forge_config,
            project_path=state["project_path"],
        )

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
