"""Integration tools — query available integrations from the system."""

from __future__ import annotations

import json
import logging

import asyncpg

from ai_forge.tools.registry import register_tool

logger = logging.getLogger(__name__)


@register_tool(
    "list_integrations",
    description="List available integrations with their capabilities.",
    parameters={"category": {"type": "string", "description": "Optional category filter."}},
)
async def list_integrations(
    pg_conn_string: str,
    category: str | None = None,
) -> str:
    """List available integrations."""
    try:
        conn = await asyncpg.connect(pg_conn_string)
        try:
            if category:
                rows = await conn.fetch(
                    """
                    SELECT id, name, type, category, description
                    FROM rootcx_system.integrations
                    WHERE category ILIKE $1
                    ORDER BY name
                    """,
                    f"%{category}%",
                )
            else:
                rows = await conn.fetch(
                    """
                    SELECT id, name, type, category, description
                    FROM rootcx_system.integrations
                    ORDER BY name
                    """
                )

            if not rows:
                return "No integrations available."

            lines = []
            for r in rows:
                lines.append(
                    f"- **{r['name']}** (type: {r['type']}, "
                    f"category: {r.get('category', 'N/A')}): "
                    f"{r.get('description', 'No description')}"
                )
            return "\n".join(lines)
        finally:
            await conn.close()
    except asyncpg.UndefinedTableError:
        return "No integrations table found — integrations not configured."
    except Exception as exc:
        logger.warning("list_integrations failed: %s", exc)
        return f"Error querying integrations: {exc}"


@register_tool(
    "get_integration_capabilities",
    description=(
        "Get detailed documentation for a specific integration "
        "including hooks, entities, actions, and usage examples."
    ),
    parameters={"type": {"type": "string", "description": "Integration type (e.g. 'gmail', 'peppol')."}},
    required=["type"],
    arg_map={"type": "integration_type"},
)
async def get_integration_capabilities(
    integration_type: str,
    pg_conn_string: str,
) -> str:
    """Get detailed capabilities for a specific integration."""
    try:
        conn = await asyncpg.connect(pg_conn_string)
        try:
            row = await conn.fetchrow(
                """
                SELECT id, name, type, category, description, capabilities
                FROM rootcx_system.integrations
                WHERE type = $1
                """,
                integration_type,
            )
            if row is None:
                return f"Integration '{integration_type}' not found."

            caps = row.get("capabilities")
            if caps:
                data = json.loads(caps) if isinstance(caps, str) else caps
                return json.dumps(data, indent=2)
            return f"Integration '{integration_type}' has no documented capabilities."
        finally:
            await conn.close()
    except asyncpg.UndefinedTableError:
        return "No integrations table found — integrations not configured."
    except Exception as exc:
        logger.warning("get_integration_capabilities failed: %s", exc)
        return f"Error: {exc}"


@register_tool(
    "search_integrations",
    description="Search integrations by keyword.",
    parameters={"query": {"type": "string", "description": "Search query."}},
    required=["query"],
)
async def search_integrations(query: str, pg_conn_string: str) -> str:
    """Search integrations by keyword."""
    try:
        conn = await asyncpg.connect(pg_conn_string)
        try:
            rows = await conn.fetch(
                """
                SELECT id, name, type, category, description
                FROM rootcx_system.integrations
                WHERE name ILIKE $1 OR description ILIKE $1 OR type ILIKE $1
                ORDER BY name
                """,
                f"%{query}%",
            )
            if not rows:
                return f"No integrations found matching '{query}'."

            lines = []
            for r in rows:
                lines.append(
                    f"- **{r['name']}** (type: {r['type']}): "
                    f"{r.get('description', 'No description')}"
                )
            return "\n".join(lines)
        finally:
            await conn.close()
    except asyncpg.UndefinedTableError:
        return "No integrations table found — integrations not configured."
    except Exception as exc:
        logger.warning("search_integrations failed: %s", exc)
        return f"Error: {exc}"
