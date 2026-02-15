"""Context manager — orchestrates multi-layer context management."""

from __future__ import annotations

__all__ = ["ContextManager"]

import logging

from langchain_aws import ChatBedrockConverse
from langchain_core.messages import AnyMessage, HumanMessage

from ai_forge.config import ForgeConfig
from ai_forge.context.summarizer import (
    estimate_tokens,
    prune_tool_outputs,
    summarize_until_fits,
    supersede_writes,
)

logger = logging.getLogger(__name__)

_SAFE_THRESHOLD = 0.80  # Summarize at 80% capacity


class ContextManager:
    """Orchestrates the multi-layer context defense system.

    Layer 1: supersede_writes — stub superseded write_file content
    Layer 2: prune_tool_outputs — truncate large tool outputs outside protected window
    Layer 3: summarize_until_fits — LLM-based summarization of old messages to 50%
    Layer 4: handle_overflow — aggressive reduction to 50% for retry after context overflow
    """

    def __init__(
        self,
        config: ForgeConfig,
        summarizer_llm: ChatBedrockConverse | None = None,
    ) -> None:
        self.config = config
        self._summarizer_llm = summarizer_llm

    @property
    def _llm(self) -> ChatBedrockConverse:
        """Lazy-create summarizer LLM only if needed (and not provided)."""
        if self._summarizer_llm is None:
            self._summarizer_llm = ChatBedrockConverse(
                model=self.config.summarizer_model_id,
                region_name=self.config.aws_region,
                max_tokens=4096,
                temperature=0,
            )
        return self._summarizer_llm

    async def prepare_messages(
        self,
        messages: list[AnyMessage],
    ) -> list[AnyMessage]:
        """Apply layers 1-3 proactively before each LLM call.

        Returns the (possibly reduced) message list.
        """
        context_limit = self.config.context_limit
        threshold = int(context_limit * _SAFE_THRESHOLD)

        # Layer 1: always apply — cheap, preserves structure
        messages = supersede_writes(messages)

        # Layer 2: always apply — cheap, only hits huge outputs outside window
        messages = prune_tool_outputs(messages)

        tokens = estimate_tokens(messages)
        if tokens <= threshold:
            return messages

        # Layer 3: summarize old messages down to 50%
        logger.info(
            "Context at ~%dK tokens (threshold %dK), summarizing...",
            tokens // 1000,
            threshold // 1000,
        )
        messages = await summarize_until_fits(
            messages,
            self._extract_existing_summary(messages),
            context_limit=context_limit,
            summarizer_llm=self._llm,
        )

        return messages

    async def handle_overflow(
        self,
        messages: list[AnyMessage],
    ) -> list[AnyMessage]:
        """Layer 4: aggressive reduction after a context overflow error.

        Targets 50% of context limit with more aggressive pruning.
        """
        logger.warning("Context overflow detected, applying aggressive reduction...")
        context_limit = self.config.context_limit

        # Apply layers 1-2 first
        messages = supersede_writes(messages)
        messages = prune_tool_outputs(messages)

        # Aggressive summarization targeting 50%
        messages = await summarize_until_fits(
            messages,
            self._extract_existing_summary(messages),
            context_limit=context_limit,
            summarizer_llm=self._llm,
            target_ratio=0.50,
        )

        tokens = estimate_tokens(messages)
        logger.info("After overflow handling: ~%dK tokens", tokens // 1000)

        return messages

    @staticmethod
    def _extract_existing_summary(messages: list[AnyMessage]) -> str | None:
        """Extract the existing conversation summary from messages, if present."""
        for msg in messages:
            if (
                isinstance(msg, HumanMessage)
                and msg.additional_kwargs.get("is_summary")
            ):
                return msg.content
        return None
