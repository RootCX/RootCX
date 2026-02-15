"""Entry point — FastAPI application with uvicorn."""

from __future__ import annotations

import logging
from contextlib import asynccontextmanager

import asyncpg
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

    @asynccontextmanager
    async def lifespan(app: FastAPI):
        # Startup: create asyncpg connection pool
        try:
            pool = await asyncpg.create_pool(
                config.pg_connection_string,
                min_size=2,
                max_size=10,
            )
            app.state.pg_pool = pool  # type: ignore[attr-defined]
            logger.info("PostgreSQL connection pool created")
        except (asyncpg.PostgresError, OSError) as exc:
            logger.warning("Failed to create PG pool (DB tools will be unavailable): %s", exc)
            app.state.pg_pool = None  # type: ignore[attr-defined]
            pool = None

        yield

        # Shutdown: close the pool
        if pool is not None:
            await pool.close()
            logger.info("PostgreSQL connection pool closed")

    app = FastAPI(title="AI Forge", version="0.1.0", lifespan=lifespan)

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
