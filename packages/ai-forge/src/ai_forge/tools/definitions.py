"""Tool schemas in Anthropic/Bedrock format — auto-generated from the registry."""

from __future__ import annotations

import ai_forge.tools  # noqa: F401 — triggers @register_tool decorators
from ai_forge.tools.registry import get_bedrock_schemas

TOOL_DEFINITIONS: list[dict] = get_bedrock_schemas()
