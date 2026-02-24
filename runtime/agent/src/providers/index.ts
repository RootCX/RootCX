import type { BaseChatModel } from "@langchain/core/language_models/chat_models";

export type ProviderConfig =
    | { type: "anthropic"; model: string }
    | { type: "openai"; model: string }
    | { type: "bedrock"; model: string; region?: string };

const PROVIDERS: Record<string, (config: ProviderConfig) => Promise<BaseChatModel>> = {
    anthropic: (c) => import("./anthropic.js").then((m) => m.create(c)),
    openai: (c) => import("./openai.js").then((m) => m.create(c)),
    bedrock: (c) => import("./bedrock.js").then((m) => m.create(c)),
};

export async function buildProvider(provider: ProviderConfig): Promise<BaseChatModel> {
    const factory = PROVIDERS[provider.type];
    if (!factory) {
        throw new Error(
            `Unknown provider type: "${provider.type}". Expected: ${Object.keys(PROVIDERS).join(", ")}`,
        );
    }
    return factory(provider);
}
