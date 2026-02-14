"""BDS component registry — static JSON lookup."""

from __future__ import annotations

import json
import logging
from pathlib import Path

from ai_forge.tools.registry import register_tool

logger = logging.getLogger(__name__)

_REGISTRY: list[dict] | None = None
_DATA_DIR = Path(__file__).parent.parent.parent.parent / "data"


def _load_registry() -> list[dict]:
    global _REGISTRY
    if _REGISTRY is not None:
        return _REGISTRY

    path = _DATA_DIR / "components.json"
    if not path.exists():
        logger.warning("components.json not found at %s", path)
        _REGISTRY = []
        return _REGISTRY

    _REGISTRY = json.loads(path.read_text(encoding="utf-8"))
    return _REGISTRY


@register_tool(
    "search_components",
    description=(
        "Search the Business Design System component library "
        "by name, category, or description."
    ),
    parameters={"query": {"type": "string", "description": "Search query (e.g. 'button', 'table', 'form')."}},
    required=["query"],
)
def search_components(query: str) -> str:
    """Search components by name, category, or description."""
    registry = _load_registry()
    q = query.lower()

    matches = []
    for comp in registry:
        name = comp.get("name", "").lower()
        category = comp.get("category", "").lower()
        description = comp.get("description", "").lower()
        if q in name or q in category or q in description:
            matches.append(
                f"- **{comp['name']}** ({comp.get('category', 'uncategorized')}): "
                f"{comp.get('description', 'No description')}"
            )

    if not matches:
        return f"No components found matching '{query}'."
    return "\n".join(matches)


@register_tool(
    "get_component_docs",
    description=(
        "Get detailed documentation for a specific BDS component "
        "including props, types, and usage examples."
    ),
    parameters={"name": {"type": "string", "description": "Component name (e.g. 'BusinessButton')."}},
    required=["name"],
)
def get_component_docs(name: str) -> str:
    """Get detailed docs for a specific component."""
    registry = _load_registry()
    name_lower = name.lower()

    for comp in registry:
        if comp.get("name", "").lower() == name_lower:
            return json.dumps(comp, indent=2)

    return f"Component '{name}' not found in the registry."
