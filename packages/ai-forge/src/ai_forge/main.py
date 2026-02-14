"""Entry point — FastAPI application with uvicorn."""

from __future__ import annotations

import logging

import uvicorn
from fastapi import FastAPI
from fastapi.middleware.cors import CORSMiddleware

from ai_forge.config import ForgeConfig
from ai_forge.server.routes import router

logger = logging.getLogger(__name__)


def create_app(config: ForgeConfig | None = None) -> FastAPI:
    """Build and return the FastAPI application."""
    if config is None:
        config = ForgeConfig.from_env()

    app = FastAPI(title="AI Forge", version="0.1.0")

    app.add_middleware(
        CORSMiddleware,
        allow_origins=["*"],
        allow_methods=["*"],
        allow_headers=["*"],
    )

    app.state.config = config  # type: ignore[attr-defined]
    app.include_router(router)

    return app


def main() -> None:
    """CLI entry point — start the uvicorn server."""
    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s %(levelname)s %(name)s: %(message)s",
    )

    config = ForgeConfig.from_env()
    app = create_app(config)

    logger.info("AI Forge starting on %s:%d", config.host, config.port)
    uvicorn.run(app, host=config.host, port=config.port, log_level="info")


if __name__ == "__main__":
    main()
