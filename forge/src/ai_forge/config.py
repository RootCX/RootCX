"""Runtime configuration — resolved from environment variables."""

from __future__ import annotations

import logging
import os
from dataclasses import dataclass

logger = logging.getLogger(__name__)


@dataclass(frozen=True)
class ForgeConfig:
    """Immutable runtime config, populated from env vars at startup."""

    # Server
    host: str = "127.0.0.1"
    port: int = 3100

    # AWS Bedrock
    aws_region: str = "us-east-1"
    aws_access_key_id: str = ""
    aws_secret_access_key: str = ""
    model_id: str = "global.anthropic.claude-opus-4-5-20251101-v1:0"
    summarizer_model_id: str = "us.anthropic.claude-haiku-4-5-20251001-v1:0"

    # LLM parameters
    llm_max_tokens: int = 16384
    llm_temperature: float = 1.0

    # PostgreSQL (Studio's embedded instance)
    pg_host: str = "localhost"
    pg_port: int = 5480
    pg_database: str = "postgres"
    pg_user: str = ""

    # Data directory (checkpoints, etc.)
    data_dir: str = ""

    # Project paths
    projects_dir: str = ""

    # Agent limits
    max_iterations: int = 50
    context_limit: int = 200_000
    max_messages: int = 200  # Hard cap on message list length

    # Project scanning limits
    max_project_files: int = 200
    max_preview_chars: int = 1000  # Per-file preview
    max_total_preview_chars: int = 120_000  # ~30K tokens total for previews

    @property
    def pg_connection_string(self) -> str:
        user_part = f"{self.pg_user}@" if self.pg_user else ""
        return f"postgresql://{user_part}{self.pg_host}:{self.pg_port}/{self.pg_database}"

    @classmethod
    def from_env(cls) -> ForgeConfig:
        """Build config from environment variables with sensible defaults.

        Warns if AWS credentials are missing (Bedrock calls will fail later).
        """
        aws_key = os.environ.get("AWS_ACCESS_KEY_ID", "")
        aws_secret = os.environ.get("AWS_SECRET_ACCESS_KEY", "")

        if not aws_key or not aws_secret:
            logger.warning(
                "AWS credentials not set (AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY). "
                "LLM calls will fail."
            )

        return cls(
            host=os.environ.get("FORGE_HOST", "127.0.0.1"),
            port=int(os.environ.get("FORGE_PORT", "3100")),
            aws_region=os.environ.get("AWS_REGION", "us-east-1"),
            aws_access_key_id=aws_key,
            aws_secret_access_key=aws_secret,
            model_id=os.environ.get(
                "FORGE_MODEL_ID",
                "global.anthropic.claude-opus-4-5-20251101-v1:0",
            ),
            summarizer_model_id=os.environ.get(
                "FORGE_SUMMARIZER_MODEL_ID",
                "us.anthropic.claude-haiku-4-5-20251001-v1:0",
            ),
            llm_max_tokens=int(os.environ.get("FORGE_LLM_MAX_TOKENS", "16384")),
            llm_temperature=float(os.environ.get("FORGE_LLM_TEMPERATURE", "1.0")),
            data_dir=os.environ.get(
                "FORGE_DATA_DIR",
                os.path.join(os.path.expanduser("~"), ".rootcx"),
            ),
            pg_host=os.environ.get("FORGE_PG_HOST", "localhost"),
            pg_port=int(os.environ.get("FORGE_PG_PORT", "5480")),
            pg_database=os.environ.get("FORGE_PG_DATABASE", "postgres"),
            pg_user=os.environ.get("FORGE_PG_USER", ""),
            projects_dir=os.environ.get("FORGE_PROJECTS_DIR", ""),
            max_iterations=int(os.environ.get("FORGE_MAX_ITERATIONS", "50")),
            context_limit=int(os.environ.get("FORGE_CONTEXT_LIMIT", "200000")),
            max_messages=int(os.environ.get("FORGE_MAX_MESSAGES", "200")),
            max_project_files=int(os.environ.get("FORGE_MAX_PROJECT_FILES", "200")),
            max_preview_chars=int(os.environ.get("FORGE_MAX_PREVIEW_CHARS", "1000")),
            max_total_preview_chars=int(os.environ.get("FORGE_MAX_TOTAL_PREVIEW_CHARS", "120000")),
        )
