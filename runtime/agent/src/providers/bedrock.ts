import type { BaseChatModel } from "@langchain/core/language_models/chat_models";
import { requireEnv } from "./env.js";

export async function create(config: { model: string; region?: string }): Promise<BaseChatModel> {
    const token = requireEnv("AWS_BEARER_TOKEN_BEDROCK");
    const { ChatBedrockConverse } = await import("@langchain/aws");
    return new ChatBedrockConverse({
        model: config.model,
        region: config.region ?? "us-east-1",
        streaming: true,
        clientOptions: {
            token: { token, expiration: new Date(Date.now() + 3_600_000) },
        },
    });
}
