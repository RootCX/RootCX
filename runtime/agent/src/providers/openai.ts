import type { BaseChatModel } from "@langchain/core/language_models/chat_models";
import { requireEnv } from "./env.js";

export async function create(config: { model: string }): Promise<BaseChatModel> {
    const { ChatOpenAI } = await import("@langchain/openai");
    return new ChatOpenAI({
        model: config.model,
        apiKey: requireEnv("OPENAI_API_KEY"),
        streaming: true,
    });
}
