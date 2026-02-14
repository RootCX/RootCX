"""Tool executor — dispatches tool calls to implementations via the registry."""

from __future__ import annotations

__all__ = ["ToolResult", "execute_tool"]

import logging
from typing import Any

from ai_forge.config import ForgeConfig
from ai_forge.graph.state import FileChange
import ai_forge.tools  # noqa: F401 — triggers @register_tool decorators
from ai_forge.tools.registry import dispatch

logger = logging.getLogger(__name__)


class ToolResult:
    """Result of a tool execution."""

    def __init__(
        self,
        output: str,
        change: FileChange | None = None,
    ):
        self.output = output
        self.change = change


async def execute_tool(
    tool_name: str,
    tool_input: dict[str, Any],
    *,
    config: ForgeConfig,
    project_path: str,
) -> ToolResult:
    """Dispatch a tool call to its implementation and return the result."""
    try:
        result = await dispatch(
            tool_name,
            tool_input,
            project_path=project_path,
            pg_conn_string=config.pg_connection_string,
        )

        if result is None:
            return ToolResult(f"Unknown tool: {tool_name}")

        # Tools that return (output, FileChange) tuples
        if isinstance(result, tuple):
            output, change = result
            return ToolResult(output, change=change)

        return ToolResult(result)

    except KeyError as exc:
        return ToolResult(f"Missing required argument for {tool_name}: {exc}")
    except Exception as exc:
        logger.exception("Tool execution failed: %s", tool_name)
        return ToolResult(f"Error executing {tool_name}: {exc}")
