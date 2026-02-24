import type { BaseChatModel } from "@langchain/core/language_models/chat_models";

export async function create(config: { model: string; api_key?: string }): Promise<BaseChatModel> {
    const apiKey = config.api_key;
    if (!apiKey) throw new Error("Missing api_key for Anthropic provider");
    const { ChatAnthropic } = await import("@langchain/anthropic");
    return new ChatAnthropic({ model: config.model, apiKey, streaming: true });
}
