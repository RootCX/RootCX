import { ChatAnthropic } from "@langchain/anthropic";
import { ChatOpenAI } from "@langchain/openai";
import { ChatBedrockConverse } from "@langchain/aws";
import type { BaseChatModel } from "@langchain/core/language_models/chat_models";

function requireEnv(name: string): string {
    const value = process.env[name];
    if (!value) {
        throw new Error(
            `Missing ${name}. Set it as a platform secret via the Runtime API: ` +
            `POST /api/v1/platform/secrets { "key": "${name}", "value": "..." }`,
        );
    }
    return value;
}

function isOpenAIModel(id: string): boolean {
    return id.startsWith("gpt-") || id.startsWith("openai/") || id.startsWith("o1") || id.startsWith("o3");
}

function isBedrockModel(id: string): boolean {
    return id.startsWith("bedrock/") || id.startsWith("us.") || id.startsWith("eu.") || id.startsWith("global.");
}

const BEDROCK_DEFAULT_MODEL = "global.anthropic.claude-opus-4-6-v1";

function detectDefaultModel(): string {
    if (process.env.ANTHROPIC_API_KEY) return "claude-sonnet-4-20250514";
    if (process.env.AWS_BEARER_TOKEN_BEDROCK) return `bedrock/${BEDROCK_DEFAULT_MODEL}`;
    if (process.env.OPENAI_API_KEY) return "gpt-4o";
    return "claude-sonnet-4-20250514";
}

export function buildProvider(modelId?: string): BaseChatModel {
    const id = modelId ?? detectDefaultModel();

    if (isBedrockModel(id)) {
        // AWS SDK picks up AWS_BEARER_TOKEN_BEDROCK automatically as Bearer token auth
        requireEnv("AWS_BEARER_TOKEN_BEDROCK");
        const region = process.env.AWS_REGION ?? "us-east-1";
        const bedrockModelId = id.replace("bedrock/", "");
        return new ChatBedrockConverse({
            model: bedrockModelId,
            region,
            streaming: true,
        });
    }

    if (isOpenAIModel(id)) {
        return new ChatOpenAI({
            model: id.replace("openai/", ""),
            apiKey: requireEnv("OPENAI_API_KEY"),
            streaming: true,
        });
    }

    return new ChatAnthropic({
        model: id,
        apiKey: requireEnv("ANTHROPIC_API_KEY"),
        streaming: true,
    });
}
