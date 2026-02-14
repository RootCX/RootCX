"""Verify node — run build verification and finalize."""

from __future__ import annotations

import logging
from dataclasses import asdict
from typing import Any

from langchain_core.runnables import RunnableConfig

from ai_forge.graph.state import BuildState
from ai_forge.server.sse import ForgeBroadcaster

logger = logging.getLogger(__name__)


async def _broadcast_completion(
    broadcaster: ForgeBroadcaster,
    project_id: str,
    *,
    success: bool,
    message: str,
    applied_changes: list,
) -> None:
    """Broadcast phase + complete events and close the stream."""
    await broadcaster.broadcast(project_id, {
        "type": "phase",
        "phase": "done" if success else "error",
    })

    await broadcaster.broadcast(project_id, {
        "type": "complete",
        "success": success,
        "message": message,
        "applied_changes": [
            {"path": c.path, "action": c.action}
            for c in applied_changes
        ],
    })

    await broadcaster.close(project_id)


async def verify_node(
    state: BuildState,
    config: RunnableConfig,
) -> dict[str, Any]:
    """Final verification — broadcast completion status."""
    broadcaster: ForgeBroadcaster = config["configurable"]["broadcaster"]
    project_id = state["project_id"]

    phase = state.get("phase", "done")
    error = state.get("error")
    success = phase == "done" and error is None

    await _broadcast_completion(
        broadcaster,
        project_id,
        success=success,
        message=state.get("message", "Build complete." if success else "Build failed."),
        applied_changes=state.get("applied_changes", []),
    )

    return {
        "success": success,
        "phase": "done" if success else "error",
    }


async def stopped_node(
    state: BuildState,
    config: RunnableConfig,
) -> dict[str, Any]:
    """Handle cancellation — clean up and notify."""
    broadcaster: ForgeBroadcaster = config["configurable"]["broadcaster"]
    project_id = state["project_id"]

    await _broadcast_completion(
        broadcaster,
        project_id,
        success=False,
        message="Build stopped by user.",
        applied_changes=state.get("applied_changes", []),
    )

    return {
        "success": False,
        "phase": "stopped",
        "message": "Build stopped by user.",
    }
