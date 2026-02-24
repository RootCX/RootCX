import type { BaseChatModel } from "@langchain/core/language_models/chat_models";
import { requireEnv } from "./env.js";

export async function create(config: { model: string }): Promise<BaseChatModel> {
    const { ChatAnthropic } = await import("@langchain/anthropic");
    return new ChatAnthropic({
        model: config.model,
        apiKey: requireEnv("ANTHROPIC_API_KEY"),
        streaming: true,
    });
}
