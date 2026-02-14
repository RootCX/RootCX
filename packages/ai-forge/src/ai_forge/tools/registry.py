"""Tool registry — single source of truth for tool definitions and dispatch."""

from __future__ import annotations

__all__ = ["REGISTRY", "register_tool", "ToolSpec"]

import inspect
from dataclasses import dataclass, field
from typing import Any, Callable


@dataclass(frozen=True)
class ToolSpec:
    """Everything needed to generate a Bedrock schema and dispatch a tool call."""

    name: str
    description: str
    fn: Callable[..., Any]
    parameters: dict[str, dict[str, str]] = field(default_factory=dict)
    required: list[str] = field(default_factory=list)
    arg_map: dict[str, str] = field(default_factory=dict)  # schema key → fn param name

    def bedrock_schema(self) -> dict:
        """Generate the Anthropic/Bedrock tool definition dict."""
        schema: dict[str, Any] = {
            "name": self.name,
            "description": self.description,
            "input_schema": {
                "type": "object",
                "properties": dict(self.parameters),
            },
        }
        if self.required:
            schema["input_schema"]["required"] = list(self.required)
        return schema


# Global registry — populated at import time by decorated tool functions.
REGISTRY: dict[str, ToolSpec] = {}


def register_tool(
    name: str,
    *,
    description: str,
    parameters: dict[str, dict[str, str]] | None = None,
    required: list[str] | None = None,
    arg_map: dict[str, str] | None = None,
) -> Callable:
    """Decorator that registers a tool function in the global registry.

    Usage::

        @register_tool(
            "read_file",
            description="Read the contents of a file from the project.",
            parameters={"path": {"type": "string", "description": "Relative path from project root."}},
            required=["path"],
        )
        def read_file(path: str, *, project_path: str) -> str: ...
    """

    def decorator(fn: Callable) -> Callable:
        spec = ToolSpec(
            name=name,
            description=description,
            fn=fn,
            parameters=parameters or {},
            required=required or [],
            arg_map=arg_map or {},
        )
        if name in REGISTRY:
            raise ValueError(f"Duplicate tool registration: {name}")
        REGISTRY[name] = spec
        return fn

    return decorator


def get_bedrock_schemas() -> list[dict]:
    """Return all registered tools as Bedrock-format schema dicts."""
    return [spec.bedrock_schema() for spec in REGISTRY.values()]


async def dispatch(
    tool_name: str,
    tool_input: dict[str, Any],
    **context: Any,
) -> Any:
    """Call a registered tool function, injecting context kwargs it accepts.

    Tool functions declare what they need via keyword arguments (e.g.
    ``project_path``, ``pg_conn_string``).  This dispatcher inspects the
    function signature and passes only the context kwargs the function
    actually accepts, plus the tool_input positional args matched by name.
    """
    spec = REGISTRY.get(tool_name)
    if spec is None:
        return None  # caller handles unknown tool

    fn = spec.fn
    sig = inspect.signature(fn)

    # Apply arg_map: remap schema keys to function param names
    mapped_input = {}
    for key, value in tool_input.items():
        mapped_key = spec.arg_map.get(key, key)
        mapped_input[mapped_key] = value

    # Build kwargs from mapped tool_input, context, and defaults
    kwargs: dict[str, Any] = {}
    for param_name, param in sig.parameters.items():
        if param_name in mapped_input:
            kwargs[param_name] = mapped_input[param_name]
        elif param_name in context:
            kwargs[param_name] = context[param_name]
        elif param.default is not inspect.Parameter.empty:
            kwargs[param_name] = param.default

    result = fn(**kwargs)
    if inspect.isawaitable(result):
        result = await result
    return result
