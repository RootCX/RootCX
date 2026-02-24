import type { BaseChatModel } from "@langchain/core/language_models/chat_models";

export async function create(config: { model: string; region?: string }): Promise<BaseChatModel> {
    // Uses the standard AWS credential chain (env vars, ~/.aws/credentials,
    // instance profile, etc.). Set AWS_ACCESS_KEY_ID + AWS_SECRET_ACCESS_KEY
    // as platform secrets, or rely on ambient credentials.
    const { ChatBedrockConverse } = await import("@langchain/aws");
    return new ChatBedrockConverse({
        model: config.model,
        region: config.region ?? "us-east-1",
        streaming: true,
    });
}
