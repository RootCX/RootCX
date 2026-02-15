"""SQLite checkpointer setup for LangGraph persistence."""

from __future__ import annotations

import logging
from pathlib import Path

import aiosqlite
from langgraph.checkpoint.sqlite.aio import AsyncSqliteSaver

from ai_forge.config import ForgeConfig

logger = logging.getLogger(__name__)


async def create_checkpointer(config: ForgeConfig) -> AsyncSqliteSaver:
    """Create and initialize the SQLite checkpointer."""
    db_path = Path(config.data_dir) / "forge_checkpoints.db"
    db_path.parent.mkdir(parents=True, exist_ok=True)
    conn = await aiosqlite.connect(str(db_path))
    checkpointer = AsyncSqliteSaver(conn)
    await checkpointer.setup()
    return checkpointer


def make_thread_id(project_id: str, conversation_id: str) -> str:
    """Build a deterministic thread ID for checkpointing."""
    return f"project:{project_id}:conv:{conversation_id}"
