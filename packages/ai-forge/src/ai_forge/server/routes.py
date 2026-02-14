"""FastAPI routes — /chat, /stream, /stop, /health, /conversations, /history."""

from __future__ import annotations

import asyncio
import json
import logging
import uuid
from collections import defaultdict
from typing import Any

import asyncpg
from fastapi import APIRouter, HTTPException, Request
from fastapi.responses import JSONResponse
from pydantic import BaseModel
from sse_starlette.sse import EventSourceResponse

from ai_forge.context.cancellation import ForgeCancellation
from ai_forge.server.sse import ForgeBroadcaster

logger = logging.getLogger(__name__)

router = APIRouter()

_broadcaster = ForgeBroadcaster()
_cancellation = ForgeCancellation()
_running_tasks: dict[str, asyncio.Task[Any]] = {}
_task_locks: dict[str, asyncio.Lock] = defaultdict(asyncio.Lock)


def _get_config(request: Request):
    return request.app.state.config


# ── Request / Response models ───────────────────────────────────────


class ChatRequest(BaseModel):
    project_id: str
    project_path: str
    prompt: str
    conversation_id: str | None = None
    app_id: str = ""


class ChatResponse(BaseModel):
    conversation_id: str
    status: str


class StopRequest(BaseModel):
    project_id: str


class ConversationOut(BaseModel):
    id: str
    project_id: str
    title: str | None
    created_at: str
    updated_at: str


class MessageOut(BaseModel):
    id: str
    role: str
    content: Any
    created_at: str


# ── Health ──────────────────────────────────────────────────────────


@router.get("/health")
async def health():
    return {"status": "ok", "service": "ai-forge", "version": "0.1.0"}


# ── Chat ────────────────────────────────────────────────────────────


@router.post("/chat", response_model=ChatResponse, status_code=202)
async def chat(body: ChatRequest, request: Request):
    """Start a build workflow. Returns 202 immediately; stream via /stream."""
    config = _get_config(request)

    if not body.project_id.strip():
        raise HTTPException(status_code=400, detail="project_id is required")
    if not body.prompt.strip():
        raise HTTPException(status_code=400, detail="prompt is required")

    conversation_id = body.conversation_id or str(uuid.uuid4())

    # Serialize cancel-and-replace per project to prevent duplicate tasks
    async with _task_locks[body.project_id]:
        if body.project_id in _running_tasks:
            _cancellation.cancel(body.project_id)
            existing = _running_tasks.pop(body.project_id, None)
            if existing and not existing.done():
                existing.cancel()
                try:
                    await existing
                except (asyncio.CancelledError, Exception):
                    pass

        _cancellation.reset(body.project_id)

        # Import here to avoid circular imports at module level
        from ai_forge.graph.workflow import run_workflow

        task = asyncio.create_task(
            run_workflow(
                config=config,
                project_id=body.project_id,
                project_path=body.project_path,
                user_prompt=body.prompt,
                conversation_id=conversation_id,
                app_id=body.app_id,
                broadcaster=_broadcaster,
                cancellation=_cancellation,
            )
        )
        _running_tasks[body.project_id] = task

    # Clean up when done (outside the lock)
    def _on_done(t: asyncio.Task[Any]) -> None:
        _running_tasks.pop(body.project_id, None)

    task.add_done_callback(_on_done)

    return ChatResponse(conversation_id=conversation_id, status="started")


# ── SSE Stream ──────────────────────────────────────────────────────


@router.get("/stream/{project_id}")
async def stream(project_id: str):
    """Server-Sent Events stream for a project's build progress."""

    async def event_generator():
        async for event in _broadcaster.subscribe(project_id):
            yield {
                "event": event.get("type", "message"),
                "data": json.dumps(event),
            }

    return EventSourceResponse(event_generator())


# ── Stop ────────────────────────────────────────────────────────────


@router.post("/stop")
async def stop(body: StopRequest):
    """Cancel a running build workflow."""
    _cancellation.cancel(body.project_id)

    task = _running_tasks.pop(body.project_id, None)
    if task and not task.done():
        task.cancel()
        await _broadcaster.broadcast(
            body.project_id,
            {"type": "phase", "phase": "stopped"},
        )
        await _broadcaster.close(body.project_id)
        return {"status": "stopped"}

    return {"status": "already_stopped"}


# ── Conversations ───────────────────────────────────────────────────


@router.get("/conversations", response_model=list[ConversationOut])
async def list_conversations(project_id: str, request: Request):
    """List conversations for a project."""
    config = _get_config(request)
    try:
        conn = await asyncpg.connect(config.pg_connection_string)
    except (asyncpg.PostgresError, OSError) as exc:
        logger.error("Database connection failed: %s", exc)
        raise HTTPException(status_code=503, detail="Database unavailable")

    try:
        rows = await conn.fetch(
            """
            SELECT id, project_id, title, created_at, updated_at
            FROM rootcx_system.forge_conversations
            WHERE project_id = $1
            ORDER BY updated_at DESC
            """,
            project_id,
        )
        return [
            ConversationOut(
                id=r["id"],
                project_id=r["project_id"],
                title=r["title"],
                created_at=r["created_at"].isoformat(),
                updated_at=r["updated_at"].isoformat(),
            )
            for r in rows
        ]
    except asyncpg.UndefinedTableError:
        return []
    except asyncpg.PostgresError as exc:
        logger.error("Query failed: %s", exc)
        raise HTTPException(status_code=503, detail="Database query failed")
    finally:
        await conn.close()


# ── History ─────────────────────────────────────────────────────────


@router.get("/history/{conversation_id}", response_model=list[MessageOut])
async def get_history(conversation_id: str, request: Request):
    """Get message history for a conversation."""
    config = _get_config(request)
    try:
        conn = await asyncpg.connect(config.pg_connection_string)
    except (asyncpg.PostgresError, OSError) as exc:
        logger.error("Database connection failed: %s", exc)
        raise HTTPException(status_code=503, detail="Database unavailable")

    try:
        rows = await conn.fetch(
            """
            SELECT id, role, content, created_at
            FROM rootcx_system.forge_messages
            WHERE conversation_id = $1
            ORDER BY created_at ASC
            """,
            conversation_id,
        )
        return [
            MessageOut(
                id=r["id"],
                role=r["role"],
                content=json.loads(r["content"]),
                created_at=r["created_at"].isoformat(),
            )
            for r in rows
        ]
    except asyncpg.UndefinedTableError:
        return []
    except asyncpg.PostgresError as exc:
        logger.error("Query failed: %s", exc)
        raise HTTPException(status_code=503, detail="Database query failed")
    finally:
        await conn.close()
