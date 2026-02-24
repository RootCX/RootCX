import type { BaseChatModel } from "@langchain/core/language_models/chat_models";

export async function create(config: { model: string; region?: string }): Promise<BaseChatModel> {
    const { ChatBedrockConverse } = await import("@langchain/aws");
    return new ChatBedrockConverse({
        model: config.model,
        region: config.region ?? "us-east-1",
        streaming: true,
    });
}
