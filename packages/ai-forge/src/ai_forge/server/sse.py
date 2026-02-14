"""SSE event broadcaster — per-project async queues."""

from __future__ import annotations

import asyncio
import logging
from collections import defaultdict
from typing import Any, AsyncGenerator

logger = logging.getLogger(__name__)

_QUEUE_MAX_SIZE = 256


class ForgeBroadcaster:
    """Manages SSE subscriptions per project_id."""

    def __init__(self) -> None:
        self._subscribers: dict[str, list[asyncio.Queue[dict[str, Any] | None]]] = (
            defaultdict(list)
        )

    async def subscribe(
        self, project_id: str
    ) -> AsyncGenerator[dict[str, Any], None]:
        """Yield SSE events for a project. Blocks until events arrive."""
        queue: asyncio.Queue[dict[str, Any] | None] = asyncio.Queue(
            maxsize=_QUEUE_MAX_SIZE,
        )
        self._subscribers[project_id].append(queue)
        try:
            while True:
                event = await queue.get()
                if event is None:
                    break
                yield event
        finally:
            try:
                self._subscribers[project_id].remove(queue)
            except ValueError:
                pass
            if not self._subscribers.get(project_id):
                self._subscribers.pop(project_id, None)

    async def broadcast(self, project_id: str, event: dict[str, Any]) -> None:
        """Send an event to all subscribers for a project.

        Drops events for slow subscribers instead of blocking.
        """
        for queue in self._subscribers.get(project_id, []):
            try:
                queue.put_nowait(event)
            except asyncio.QueueFull:
                logger.warning("SSE queue full for project %s, dropping event", project_id)

    async def close(self, project_id: str) -> None:
        """Send sentinel to all subscribers, ending their streams."""
        for queue in self._subscribers.get(project_id, []):
            try:
                queue.put_nowait(None)
            except asyncio.QueueFull:
                # Force: drain one and push sentinel
                try:
                    queue.get_nowait()
                except asyncio.QueueEmpty:
                    pass
                try:
                    queue.put_nowait(None)
                except asyncio.QueueFull:
                    pass
