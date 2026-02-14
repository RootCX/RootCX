"""LangGraph workflow definition and runner."""

from __future__ import annotations

import asyncio
import json
import logging
import uuid
from typing import Any, Literal

import asyncpg
from langchain_core.messages import AIMessage
from langgraph.graph import END, START, StateGraph

from ai_forge.config import ForgeConfig
from ai_forge.context.cancellation import ForgeCancellation
from ai_forge.context.checkpointer import create_checkpointer, make_thread_id
from ai_forge.graph.nodes.agent import agent_node
from ai_forge.graph.nodes.load_project import load_project_node
from ai_forge.graph.nodes.tools import tools_node
from ai_forge.graph.nodes.verify import stopped_node, verify_node
from ai_forge.graph.state import BuildState
from ai_forge.server.sse import ForgeBroadcaster

logger = logging.getLogger(__name__)


# ── Routing functions ───────────────────────────────────────────────


def route_after_agent(state: BuildState) -> Literal["tools", "verify", "stopped"]:
    """Decide next step after the agent node."""
    if state.get("phase") == "stopped":
        return "stopped"

    if state.get("phase") == "error":
        return "verify"

    # Check iteration limit
    if state.get("iteration", 0) >= state.get("max_iterations", 50):
        return "verify"

    # If the AI made tool calls, execute them
    messages = state.get("messages", [])
    if messages:
        last = messages[-1]
        if isinstance(last, AIMessage) and last.tool_calls:
            return "tools"

    # No tool calls — agent is done
    return "verify"


def route_after_tools(state: BuildState) -> Literal["agent", "stopped"]:
    """After tools execute, go back to agent (or stop if cancelled)."""
    if state.get("phase") == "stopped":
        return "stopped"
    return "agent"


# ── Graph construction ──────────────────────────────────────────────


def build_graph() -> StateGraph:
    """Construct the LangGraph StateGraph."""
    graph = StateGraph(BuildState)

    graph.add_node("load_project", load_project_node)
    graph.add_node("agent", agent_node)
    graph.add_node("tools", tools_node)
    graph.add_node("verify", verify_node)
    graph.add_node("stopped", stopped_node)

    graph.add_edge(START, "load_project")
    graph.add_edge("load_project", "agent")
    graph.add_conditional_edges("agent", route_after_agent)
    graph.add_conditional_edges("tools", route_after_tools)
    graph.add_edge("verify", END)
    graph.add_edge("stopped", END)

    return graph


# ── Conversation persistence ───────────────────────────────────────


async def _ensure_conversation(
    conn: asyncpg.Connection,
    conversation_id: str,
    project_id: str,
) -> None:
    """Create conversation record if it doesn't exist."""
    await conn.execute(
        """
        INSERT INTO rootcx_system.forge_conversations (id, project_id, title)
        VALUES ($1, $2, $3)
        ON CONFLICT (id) DO UPDATE SET updated_at = now()
        """,
        conversation_id,
        project_id,
        "New conversation",
    )


async def _save_message(
    conn: asyncpg.Connection,
    conversation_id: str,
    role: str,
    content: Any,
) -> None:
    """Persist a message to the database."""
    await conn.execute(
        """
        INSERT INTO rootcx_system.forge_messages (id, conversation_id, role, content)
        VALUES ($1, $2, $3, $4)
        """,
        str(uuid.uuid4()),
        conversation_id,
        role,
        json.dumps(content, default=str),
    )


# ── Main runner ─────────────────────────────────────────────────────


async def run_workflow(
    *,
    config: ForgeConfig,
    project_id: str,
    project_path: str,
    user_prompt: str,
    conversation_id: str,
    app_id: str,
    broadcaster: ForgeBroadcaster,
    cancellation: ForgeCancellation,
) -> None:
    """Run the full build workflow — called as an asyncio task."""
    logger.info(
        "Starting workflow for project=%s conv=%s",
        project_id,
        conversation_id,
    )

    try:
        # Persist conversation
        try:
            conn = await asyncpg.connect(config.pg_connection_string)
            try:
                await _ensure_conversation(conn, conversation_id, project_id)
                await _save_message(conn, conversation_id, "user", user_prompt)
            finally:
                await conn.close()
        except Exception as exc:
            logger.warning("Failed to persist conversation: %s", exc)

        # Build and compile graph
        graph = build_graph()

        try:
            checkpointer = await create_checkpointer(config)
            compiled = graph.compile(checkpointer=checkpointer)
        except Exception as exc:
            logger.warning("Checkpointer setup failed, running without persistence: %s", exc)
            compiled = graph.compile()

        thread_id = make_thread_id(project_id, conversation_id)

        initial_state: BuildState = {
            "project_id": project_id,
            "project_path": project_path,
            "user_prompt": user_prompt,
            "conversation_id": conversation_id,
            "app_id": app_id or project_id,
            "messages": [],
            "conversation_summary": None,
            "phase": "analyzing",
            "thinking": "",
            "plan": [],
            "applied_changes": [],
            "errors": [],
            "iteration": 0,
            "max_iterations": config.max_iterations,
            "success": False,
            "message": "",
            "error": None,
        }

        run_config = {
            "configurable": {
                "thread_id": thread_id,
                "forge_config": config,
                "broadcaster": broadcaster,
                "cancellation": cancellation,
            }
        }

        # Run the graph
        final_state = await compiled.ainvoke(initial_state, config=run_config)

        # Persist assistant response
        try:
            conn = await asyncpg.connect(config.pg_connection_string)
            try:
                summary = final_state.get("message", "Build complete.")
                await _save_message(conn, conversation_id, "assistant", summary)
            finally:
                await conn.close()
        except Exception as exc:
            logger.warning("Failed to persist response: %s", exc)

    except asyncio.CancelledError:
        logger.info("Workflow cancelled for project=%s", project_id)
        await broadcaster.broadcast(project_id, {
            "type": "phase",
            "phase": "stopped",
        })
        await broadcaster.close(project_id)

    except Exception as exc:
        logger.exception("Workflow failed for project=%s", project_id)
        await broadcaster.broadcast(project_id, {
            "type": "error",
            "message": str(exc),
        })
        await broadcaster.broadcast(project_id, {
            "type": "phase",
            "phase": "error",
        })
        await broadcaster.close(project_id)
