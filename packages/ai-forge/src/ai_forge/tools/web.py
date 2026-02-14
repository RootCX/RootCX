"""Web tools — browse URLs and search the web."""

from __future__ import annotations

import logging
import re

import httpx

from ai_forge.tools.registry import register_tool

logger = logging.getLogger(__name__)

_TIMEOUT = 15.0
_MAX_CONTENT_LENGTH = 50_000


def _html_to_text(html: str) -> str:
    """Minimal HTML to plain text conversion."""
    # Strip script/style tags
    text = re.sub(r"<(script|style)[^>]*>.*?</\1>", "", html, flags=re.DOTALL | re.IGNORECASE)
    # Strip tags
    text = re.sub(r"<[^>]+>", " ", text)
    # Collapse whitespace
    text = re.sub(r"\s+", " ", text).strip()
    return text[:_MAX_CONTENT_LENGTH]


@register_tool(
    "web_browse",
    description="Fetch a URL and return its content as markdown.",
    parameters={"url": {"type": "string", "description": "The URL to fetch."}},
    required=["url"],
)
async def web_browse(url: str) -> str:
    """Fetch a URL and return content as text."""
    try:
        async with httpx.AsyncClient(
            follow_redirects=True,
            timeout=_TIMEOUT,
        ) as client:
            resp = await client.get(url)
            resp.raise_for_status()

            content_type = resp.headers.get("content-type", "")
            if "html" in content_type:
                return _html_to_text(resp.text)
            return resp.text[:_MAX_CONTENT_LENGTH]
    except Exception as exc:
        return f"Error fetching {url}: {exc}"


