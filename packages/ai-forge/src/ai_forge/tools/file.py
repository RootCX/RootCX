"""File tools — read, write, list, delete on the local project directory."""

from __future__ import annotations

__all__ = ["read_file", "write_file", "list_files", "delete_file"]

import os
from pathlib import Path

from ai_forge.graph.state import FileChange
from ai_forge.tools.registry import register_tool

_MAX_READ_BYTES = 10 * 1024 * 1024  # 10 MB


def _resolve(project_path: str, relative: str) -> Path:
    """Resolve and validate a relative path inside the project root."""
    base = Path(project_path).resolve()
    target = (base / relative).resolve()
    if not str(target).startswith(str(base)):
        raise ValueError(f"Path escapes project root: {relative}")
    return target


@register_tool(
    "read_file",
    description="Read the contents of a file from the project.",
    parameters={"path": {"type": "string", "description": "Relative path from project root."}},
    required=["path"],
)
def read_file(
    path: str,
    *,
    project_path: str,
) -> str:
    """Read a file from disk."""
    target = _resolve(project_path, path)
    if not target.exists():
        return f"Error: file not found: {path}"
    try:
        size = target.stat().st_size
        if size > _MAX_READ_BYTES:
            return f"Error: file too large ({size:,} bytes, limit is {_MAX_READ_BYTES:,}): {path}"
        return target.read_text(encoding="utf-8")
    except (UnicodeDecodeError, PermissionError) as exc:
        return f"Error reading {path}: {exc}"


@register_tool(
    "write_file",
    description="Create or overwrite a file in the project.",
    parameters={
        "path": {"type": "string", "description": "Relative path from project root."},
        "content": {"type": "string", "description": "Full file content to write."},
    },
    required=["path", "content"],
)
def write_file(
    path: str,
    content: str,
    *,
    project_path: str,
) -> tuple[str, FileChange]:
    """Write file to disk, return confirmation message and change record."""
    target = _resolve(project_path, path)
    existed = target.exists()
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(content, encoding="utf-8")

    action = "update" if existed else "create"
    change = FileChange(path=path, action=action)
    return f"Written: {path}", change


@register_tool(
    "list_files",
    description="List all files in the project directory tree.",
    parameters={
        "directory": {"type": "string", "description": "Subdirectory to list (default: project root).", "default": ""},
    },
)
def list_files(
    directory: str = "",
    *,
    project_path: str,
) -> str:
    """List files in a project directory."""
    base = _resolve(project_path, directory)
    if not base.exists():
        return f"Error: directory not found: {directory}"

    result: list[str] = []
    for root, _dirs, filenames in os.walk(base):
        for name in sorted(filenames):
            full = Path(root) / name
            rel = full.relative_to(Path(project_path).resolve())
            result.append(str(rel))

    if not result:
        return "No files found."
    return "\n".join(result)


@register_tool(
    "delete_file",
    description="Delete a file from the project.",
    parameters={"path": {"type": "string", "description": "Relative path from project root."}},
    required=["path"],
)
def delete_file(
    path: str,
    *,
    project_path: str,
) -> tuple[str, FileChange]:
    """Delete a file from disk."""
    target = _resolve(project_path, path)
    if target.exists():
        target.unlink()

    change = FileChange(path=path, action="delete")
    return f"Deleted: {path}", change
