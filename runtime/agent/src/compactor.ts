import {
    SystemMessage,
    HumanMessage,
    AIMessage,
    type BaseMessage,
} from "@langchain/core/messages";
import type { BaseChatModel } from "@langchain/core/language_models/chat_models";
import { estimateTokens } from "./token-counter.js";

const COMPACT_THRESHOLD = 0.8;

interface CompactionResult {
    messages: BaseMessage[];
    compacted: boolean;
    summary?: string;
}

export async function compactIfNeeded(
    messages: BaseMessage[],
    maxContextTokens: number,
    keepRecentMessages: number,
    model: BaseChatModel,
): Promise<CompactionResult> {
    const totalTokens = messages.reduce((sum, m) => sum + estimateTokens(m), 0);

    if (totalTokens < maxContextTokens * COMPACT_THRESHOLD) {
        return { messages, compacted: false };
    }

    // Need at least some messages to summarize
    if (messages.length <= keepRecentMessages + 1) {
        return { messages, compacted: false };
    }

    const toSummarize = messages.slice(0, -keepRecentMessages);
    const toKeep = messages.slice(-keepRecentMessages);

    const summaryText = await summarize(toSummarize, model);
    const summaryMessage = new SystemMessage(
        `[Conversation summary]\n${summaryText}`
    );

    return {
        messages: [summaryMessage, ...toKeep],
        compacted: true,
        summary: summaryText,
    };
}

async function summarize(messages: BaseMessage[], model: BaseChatModel): Promise<string> {
    const formatted = messages.map((m) => {
        const role = m instanceof HumanMessage ? "User"
            : m instanceof AIMessage ? "Assistant"
            : "System";
        const content = typeof m.content === "string" ? m.content : JSON.stringify(m.content);
        return `--- ${role} ---\n${content}`;
    }).join("\n\n");

    const prompt = new HumanMessage(
        `Summarize this conversation. Preserve all key decisions, entity names, data references, current task state, and pending work. Be concise but complete.\n\n${formatted}`
    );

    const response = await model.invoke([
        new SystemMessage("You are a conversation summarizer. Output a concise multi-paragraph summary."),
        prompt,
    ]);

    return typeof response.content === "string"
        ? response.content
        : JSON.stringify(response.content);
}
