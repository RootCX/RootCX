import type { BaseMessage } from "@langchain/core/messages";

// chars/4 heuristic — swap for tiktoken if accuracy matters
export function estimateTokens(message: BaseMessage): number {
    const content = typeof message.content === "string"
        ? message.content
        : JSON.stringify(message.content);
    return Math.ceil(content.length / 4);
}
