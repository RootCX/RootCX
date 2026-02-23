import { StateGraph, MessagesAnnotation } from "@langchain/langgraph";
import { ToolNode } from "@langchain/langgraph/prebuilt";
import type { BaseChatModel } from "@langchain/core/language_models/chat_models";
import type { StructuredToolInterface } from "@langchain/core/tools";

export function buildDefaultGraph(model: BaseChatModel, tools: StructuredToolInterface[]) {
    const bound = model.bindTools(tools);
    const toolNode = new ToolNode(tools);

    async function agent(state: typeof MessagesAnnotation.State) {
        return { messages: [await bound.invoke(state.messages)] };
    }

    function route(state: typeof MessagesAnnotation.State) {
        const last = state.messages.at(-1);
        const calls = (last as { tool_calls?: unknown[] } | undefined)?.tool_calls;
        return calls?.length ? "tools" : "__end__";
    }

    return new StateGraph(MessagesAnnotation)
        .addNode("agent", agent)
        .addNode("tools", toolNode)
        .addEdge("__start__", "agent")
        .addConditionalEdges("agent", route)
        .addEdge("tools", "agent")
        .compile();
}
