"""App schema and integration tools — asyncpg queries against rootcx_system."""

from __future__ import annotations

import json
import logging

import asyncpg

from ai_forge.tools.registry import register_tool

logger = logging.getLogger(__name__)


@register_tool(
    "list_installed_apps",
    description="List all installed apps in the RootCX system.",
    parameters={},
)
async def list_installed_apps(pg_conn_string: str) -> str:
    """List all installed apps."""
    try:
        conn = await asyncpg.connect(pg_conn_string)
        try:
            rows = await conn.fetch(
                "SELECT id, name, version, status FROM rootcx_system.apps ORDER BY name"
            )
            if not rows:
                return "No apps installed."
            lines = []
            for r in rows:
                lines.append(f"- **{r['name']}** (id: {r['id']}, v{r['version']}, {r['status']})")
            return "\n".join(lines)
        finally:
            await conn.close()
    except Exception as exc:
        logger.warning("list_installed_apps failed: %s", exc)
        return f"Error querying apps: {exc}"


@register_tool(
    "get_app_schema",
    description=(
        "Get the full schema for an installed app "
        "including entities, fields, and relationships."
    ),
    parameters={
        "app_id": {"type": "string", "description": "The app ID to get schema for."},
    },
    required=["app_id"],
)
async def get_app_schema(app_id: str, pg_conn_string: str) -> str:
    """Get the full manifest/schema for an app."""
    try:
        conn = await asyncpg.connect(pg_conn_string)
        try:
            row = await conn.fetchrow(
                "SELECT id, name, version, manifest FROM rootcx_system.apps WHERE id = $1",
                app_id,
            )
            if row is None:
                return f"App '{app_id}' not found."

            manifest = row["manifest"]
            if manifest is None:
                return f"App '{app_id}' has no manifest."

            data = json.loads(manifest) if isinstance(manifest, str) else manifest
            return json.dumps(data, indent=2)
        finally:
            await conn.close()
    except Exception as exc:
        logger.warning("get_app_schema failed: %s", exc)
        return f"Error querying app schema: {exc}"
