"""Tool factory — creates LangChain @tool instances with bound context via closures."""

from __future__ import annotations

__all__ = ["make_tools"]

import asyncio
import json
import logging
import os
import re
from pathlib import Path
from typing import Optional

import asyncpg
import httpx
from langchain_core.tools import tool

from ai_forge.graph.state import FileChange

logger = logging.getLogger(__name__)

# ── Shared constants ────────────────────────────────────────────────

_MAX_READ_BYTES = 10 * 1024 * 1024  # 10 MB
_BUILD_TIMEOUT = 120  # seconds
_WEB_TIMEOUT = 15.0
_MAX_CONTENT_LENGTH = 50_000

# Directories to skip when listing files
_SKIP_DIRS = {"node_modules", "target", ".git", "dist", "build", "__pycache__", ".next", ".nuxt"}

# BDS component registry (lazy-loaded)
_COMPONENT_REGISTRY: list[dict] | None = None
_DATA_DIR = Path(__file__).parent.parent.parent.parent / "data"


def _load_component_registry() -> list[dict]:
    global _COMPONENT_REGISTRY
    if _COMPONENT_REGISTRY is not None:
        return _COMPONENT_REGISTRY
    path = _DATA_DIR / "components.json"
    if not path.exists():
        logger.warning("components.json not found at %s", path)
        _COMPONENT_REGISTRY = []
        return _COMPONENT_REGISTRY
    _COMPONENT_REGISTRY = json.loads(path.read_text(encoding="utf-8"))
    return _COMPONENT_REGISTRY


# ── Factory ─────────────────────────────────────────────────────────


def make_tools(project_path: str, pg_pool: asyncpg.Pool | None) -> list:
    """Create all tools with project_path and pg_pool bound via closures.

    Returns a list of LangChain BaseTool instances ready for bind_tools().
    """

    def _resolve(relative: str) -> Path:
        """Resolve and validate a relative path inside the project root."""
        base = Path(project_path).resolve()
        target = (base / relative).resolve()
        if not str(target).startswith(str(base)):
            raise ValueError(f"Path escapes project root: {relative}")
        return target

    # ── File tools ──────────────────────────────────────────────

    @tool
    def read_file(path: str) -> str:
        """Read the contents of a file from the project. Path is relative to project root."""
        target = _resolve(path)
        if not target.exists():
            return f"Error: file not found: {path}"
        try:
            size = target.stat().st_size
            if size > _MAX_READ_BYTES:
                return f"Error: file too large ({size:,} bytes, limit is {_MAX_READ_BYTES:,}): {path}"
            return target.read_text(encoding="utf-8")
        except (UnicodeDecodeError, PermissionError) as exc:
            return f"Error reading {path}: {exc}"

    @tool
    def write_file(path: str, content: str) -> str:
        """Create or overwrite a file in the project. Path is relative to project root."""
        target = _resolve(path)
        existed = target.exists()
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content, encoding="utf-8")

        action = "update" if existed else "create"
        # Encode change info in output for the tools node to parse
        return json.dumps({
            "message": f"Written: {path}",
            "change": {"path": path, "action": action},
        })

    @tool
    def list_files(directory: str = "") -> str:
        """List all files in the project directory tree. Skips node_modules, target, .git, etc."""
        base = _resolve(directory)
        if not base.exists():
            return f"Error: directory not found: {directory}"

        result: list[str] = []
        for root_dir, dirnames, filenames in os.walk(base):
            # Skip standard large/non-useful directories
            dirnames[:] = [d for d in dirnames if d not in _SKIP_DIRS]
            for name in sorted(filenames):
                full = Path(root_dir) / name
                rel = full.relative_to(Path(project_path).resolve())
                result.append(str(rel))

        if not result:
            return "No files found."
        return "\n".join(result)

    @tool
    def delete_file(path: str) -> str:
        """Delete a file from the project. Path is relative to project root."""
        target = _resolve(path)
        if target.exists():
            target.unlink()

        return json.dumps({
            "message": f"Deleted: {path}",
            "change": {"path": path, "action": "delete"},
        })

    # ── Build tools ─────────────────────────────────────────────

    @tool
    async def verify_build() -> str:
        """Run the full build toolchain (cargo build + vite build) on the generated project to check for errors."""
        root = Path(project_path).resolve()
        results: list[str] = []

        async def _run_command(cmd: list[str], cwd: str) -> tuple[int, str, str]:
            proc = await asyncio.create_subprocess_exec(
                *cmd,
                cwd=cwd,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
            )
            try:
                stdout, stderr = await asyncio.wait_for(
                    proc.communicate(), timeout=_BUILD_TIMEOUT
                )
            except asyncio.TimeoutError:
                proc.kill()
                return -1, "", f"Build timed out after {_BUILD_TIMEOUT}s"
            return (
                proc.returncode or 0,
                stdout.decode("utf-8", errors="replace"),
                stderr.decode("utf-8", errors="replace"),
            )

        # 1. Cargo build (Rust backend)
        tauri_dir = root / "src-tauri"
        if tauri_dir.exists():
            code, stdout, stderr = await _run_command(
                ["cargo", "build"], cwd=str(tauri_dir)
            )
            if code != 0:
                error_lines = [
                    line for line in stderr.splitlines() if "error" in line.lower()
                ]
                error_summary = (
                    "\n".join(error_lines[:20]) if error_lines else stderr[-2000:]
                )
                results.append(f"CARGO BUILD FAILED (exit {code}):\n{error_summary}")
            else:
                results.append("cargo build: OK")
        else:
            results.append("WARNING: src-tauri/ not found, skipping cargo build")

        # 2. Frontend build (Vite/React)
        if (root / "package.json").exists():
            code, stdout, stderr = await _run_command(
                ["npm", "run", "build"], cwd=str(root)
            )
            if code != 0:
                error_output = stderr or stdout
                error_lines = error_output.splitlines()[-30:]
                results.append(
                    f"FRONTEND BUILD FAILED (exit {code}):\n" + "\n".join(error_lines)
                )
            else:
                results.append("npm run build: OK")
        else:
            results.append("WARNING: package.json not found, skipping frontend build")

        return "\n\n".join(results)

    # ── App/schema tools ────────────────────────────────────────

    @tool
    async def list_installed_apps() -> str:
        """List all installed apps in the RootCX system."""
        if pg_pool is None:
            return "Database not available."
        try:
            async with pg_pool.acquire() as conn:
                rows = await conn.fetch(
                    "SELECT id, name, version, status FROM rootcx_system.apps ORDER BY name"
                )
                if not rows:
                    return "No apps installed."
                lines = []
                for r in rows:
                    lines.append(
                        f"- **{r['name']}** (id: {r['id']}, v{r['version']}, {r['status']})"
                    )
                return "\n".join(lines)
        except Exception as exc:
            logger.warning("list_installed_apps failed: %s", exc)
            return f"Error querying apps: {exc}"

    @tool
    async def get_app_schema(app_id: str) -> str:
        """Get the full schema for an installed app including entities, fields, and relationships."""
        if pg_pool is None:
            return "Database not available."
        try:
            async with pg_pool.acquire() as conn:
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
        except Exception as exc:
            logger.warning("get_app_schema failed: %s", exc)
            return f"Error querying app schema: {exc}"

    # ── Component tools ─────────────────────────────────────────

    @tool
    def search_components(query: str) -> str:
        """Search the Business Design System component library by name, category, or description."""
        registry = _load_component_registry()
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

    @tool
    def get_component_docs(name: str) -> str:
        """Get detailed documentation for a specific BDS component including props, types, and usage examples."""
        registry = _load_component_registry()
        name_lower = name.lower()
        for comp in registry:
            if comp.get("name", "").lower() == name_lower:
                return json.dumps(comp, indent=2)
        return f"Component '{name}' not found in the registry."

    # ── Web tools ───────────────────────────────────────────────

    @tool
    async def web_browse(url: str) -> str:
        """Fetch a URL and return its content as text."""
        try:
            async with httpx.AsyncClient(
                follow_redirects=True, timeout=_WEB_TIMEOUT
            ) as client:
                resp = await client.get(url)
                resp.raise_for_status()
                content_type = resp.headers.get("content-type", "")
                if "html" in content_type:
                    return _html_to_text(resp.text)
                return resp.text[:_MAX_CONTENT_LENGTH]
        except Exception as exc:
            return f"Error fetching {url}: {exc}"

    # ── Integration tools ───────────────────────────────────────

    @tool
    async def list_integrations(category: Optional[str] = None) -> str:
        """List available integrations with their capabilities. Optionally filter by category."""
        if pg_pool is None:
            return "Database not available."
        try:
            async with pg_pool.acquire() as conn:
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
        except asyncpg.UndefinedTableError:
            return "No integrations table found — integrations not configured."
        except Exception as exc:
            logger.warning("list_integrations failed: %s", exc)
            return f"Error querying integrations: {exc}"

    @tool
    async def get_integration_capabilities(integration_type: str) -> str:
        """Get detailed documentation for a specific integration including hooks, entities, actions, and usage examples."""
        if pg_pool is None:
            return "Database not available."
        try:
            async with pg_pool.acquire() as conn:
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
        except asyncpg.UndefinedTableError:
            return "No integrations table found — integrations not configured."
        except Exception as exc:
            logger.warning("get_integration_capabilities failed: %s", exc)
            return f"Error: {exc}"

    @tool
    async def search_integrations(query: str) -> str:
        """Search integrations by keyword across name, type, and description."""
        if pg_pool is None:
            return "Database not available."
        try:
            async with pg_pool.acquire() as conn:
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
        except asyncpg.UndefinedTableError:
            return "No integrations table found — integrations not configured."
        except Exception as exc:
            logger.warning("search_integrations failed: %s", exc)
            return f"Error: {exc}"

    # ── Return all tools ────────────────────────────────────────

    return [
        read_file,
        write_file,
        list_files,
        delete_file,
        verify_build,
        list_installed_apps,
        get_app_schema,
        search_components,
        get_component_docs,
        web_browse,
        list_integrations,
        get_integration_capabilities,
        search_integrations,
    ]


def _html_to_text(html: str) -> str:
    """Minimal HTML to plain text conversion."""
    text = re.sub(
        r"<(script|style)[^>]*>.*?</\1>", "", html, flags=re.DOTALL | re.IGNORECASE
    )
    text = re.sub(r"<[^>]+>", " ", text)
    text = re.sub(r"\s+", " ", text).strip()
    return text[:_MAX_CONTENT_LENGTH]
