"""Build verification — runs the real toolchain on generated projects."""

from __future__ import annotations

import asyncio
import logging
from pathlib import Path

from ai_forge.tools.registry import register_tool

logger = logging.getLogger(__name__)

_BUILD_TIMEOUT = 120  # seconds


async def _run_command(cmd: list[str], cwd: str) -> tuple[int, str, str]:
    """Run a subprocess and return (returncode, stdout, stderr)."""
    proc = await asyncio.create_subprocess_exec(
        *cmd,
        cwd=cwd,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE,
    )
    try:
        stdout, stderr = await asyncio.wait_for(
            proc.communicate(),
            timeout=_BUILD_TIMEOUT,
        )
    except asyncio.TimeoutError:
        proc.kill()
        return -1, "", f"Build timed out after {_BUILD_TIMEOUT}s"

    return (
        proc.returncode or 0,
        stdout.decode("utf-8", errors="replace"),
        stderr.decode("utf-8", errors="replace"),
    )


@register_tool(
    "verify_build",
    description=(
        "Run the full build toolchain (cargo build + vite build) "
        "on the generated project to check for errors."
    ),
    parameters={},
)
async def verify_build(project_path: str) -> str:
    """Run cargo build + npm run build on the generated Tauri project."""
    root = Path(project_path).resolve()
    results: list[str] = []

    # 1. Cargo build (Rust backend)
    tauri_dir = root / "src-tauri"
    if tauri_dir.exists():
        logger.info("Running cargo build in %s", tauri_dir)
        code, stdout, stderr = await _run_command(
            ["cargo", "build"],
            cwd=str(tauri_dir),
        )
        if code != 0:
            # Extract the most useful error lines
            error_lines = [
                line for line in stderr.splitlines()
                if "error" in line.lower()
            ]
            error_summary = "\n".join(error_lines[:20]) if error_lines else stderr[-2000:]
            results.append(f"CARGO BUILD FAILED (exit {code}):\n{error_summary}")
        else:
            results.append("cargo build: OK")
    else:
        results.append("WARNING: src-tauri/ not found, skipping cargo build")

    # 2. Frontend build (Vite/React)
    if (root / "package.json").exists():
        logger.info("Running npm run build in %s", root)
        code, stdout, stderr = await _run_command(
            ["npm", "run", "build"],
            cwd=str(root),
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
