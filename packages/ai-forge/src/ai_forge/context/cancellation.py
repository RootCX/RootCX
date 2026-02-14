"""Cancellation support using asyncio.Event."""

from __future__ import annotations

import asyncio


class ForgeCancellation:
    """Per-project cancellation flags."""

    def __init__(self) -> None:
        self._events: dict[str, asyncio.Event] = {}

    def cancel(self, project_id: str) -> None:
        """Signal cancellation for a project."""
        evt = self._events.get(project_id)
        if evt is None:
            evt = asyncio.Event()
            self._events[project_id] = evt
        evt.set()

    def is_cancelled(self, project_id: str) -> bool:
        evt = self._events.get(project_id)
        return evt.is_set() if evt else False

    def reset(self, project_id: str) -> None:
        """Clear cancellation flag for a fresh run."""
        evt = self._events.get(project_id)
        if evt is not None:
            evt.clear()

    def get_event(self, project_id: str) -> asyncio.Event:
        """Return (or create) the event so callers can await it."""
        if project_id not in self._events:
            self._events[project_id] = asyncio.Event()
        return self._events[project_id]
