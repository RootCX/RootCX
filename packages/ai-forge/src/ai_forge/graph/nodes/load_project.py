"""Load project node — scans files, builds system prompt + user message."""

from __future__ import annotations

import logging
import os
from pathlib import Path

from langchain_core.messages import HumanMessage, SystemMessage

from ai_forge.graph.state import BuildState
from ai_forge.prompt.system import build_system_prompt

logger = logging.getLogger(__name__)

# File extensions worth showing previews for
_TEXT_EXTENSIONS = {
    ".ts", ".tsx", ".js", ".jsx", ".json", ".toml", ".yaml", ".yml",
    ".rs", ".html", ".css", ".scss", ".md", ".txt", ".sql", ".sh",
    ".cfg", ".ini", ".env.example",
}

# Files to skip entirely (too large, not useful for context)
_SKIP_FILES = {"Cargo.lock", "package-lock.json", "pnpm-lock.yaml", "yarn.lock"}

# Directories to skip
_SKIP_DIRS = {"node_modules", "target", ".git", "dist", "build", "__pycache__"}

_MAX_FILES = 200
_MAX_PREVIEW_CHARS = 1000  # Per-file preview limit
_MAX_TOTAL_PREVIEW_CHARS = 120_000  # ~30K tokens total for all previews


def load_project_node(state: BuildState) -> dict:
    """Scan the project directory, build static system prompt + user message with file previews."""
    project_path = state["project_path"]
    app_id = state.get("app_id", "")
    user_prompt = state.get("user_prompt", "")

    logger.info("Loading project from %s", project_path)

    # Ensure project directory exists
    Path(project_path).mkdir(parents=True, exist_ok=True)

    # Scan project files — collect paths and truncated previews
    root = Path(project_path).resolve()
    file_entries: list[str] = []  # "path" or "path (preview)"
    total_preview_chars = 0
    count = 0

    for dirpath, dirnames, filenames in os.walk(root):
        # Skip non-project directories
        dirnames[:] = [d for d in dirnames if d not in _SKIP_DIRS]

        for name in sorted(filenames):
            if count >= _MAX_FILES:
                break

            full = Path(dirpath) / name
            rel = str(full.relative_to(root))
            ext = full.suffix.lower()

            if name in _SKIP_FILES:
                file_entries.append(f"  {rel}  (skipped — large lock file)")
                count += 1
                continue

            # Try to read a preview for text files if we have budget
            if (
                ext in _TEXT_EXTENSIONS
                and total_preview_chars < _MAX_TOTAL_PREVIEW_CHARS
            ):
                try:
                    size = full.stat().st_size
                    if size <= _MAX_PREVIEW_CHARS:
                        preview = full.read_text(encoding="utf-8")
                    elif size <= 100_000:
                        preview = full.read_text(encoding="utf-8")[:_MAX_PREVIEW_CHARS] + "\n... (truncated)"
                    else:
                        file_entries.append(f"  {rel}  ({size:,} bytes)")
                        count += 1
                        continue

                    total_preview_chars += len(preview)
                    file_entries.append(f"  {rel}:\n```\n{preview}\n```")
                except (UnicodeDecodeError, PermissionError, OSError):
                    file_entries.append(f"  {rel}  (binary/unreadable)")
            else:
                try:
                    size = full.stat().st_size
                    file_entries.append(f"  {rel}  ({size:,} bytes)")
                except OSError:
                    file_entries.append(f"  {rel}")

            count += 1

    # Build static system prompt (no file listing, no summary)
    system_content = build_system_prompt(
        app_id=app_id,
        project_path=project_path,
    )
    system_msg = SystemMessage(content=system_content)

    # Build user message with file context + prompt
    if file_entries:
        file_context = "\n".join(file_entries)
        user_content = (
            f"## Current Project Files\n\n{file_context}\n\n"
            f"---\n\n## Request\n\n{user_prompt}"
        )
    else:
        user_content = (
            f"## Current Project Files\n\n(empty project — no files yet)\n\n"
            f"---\n\n## Request\n\n{user_prompt}"
        )

    user_msg = HumanMessage(content=user_content)

    logger.info("Scanned %d files from project (preview budget: %d chars used)", count, total_preview_chars)

    return {
        "messages": [system_msg, user_msg],
        "phase": "analyzing",
    }
