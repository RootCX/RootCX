import { StateGraph, MessagesAnnotation } from "@langchain/langgraph";
import { ToolNode } from "@langchain/langgraph/prebuilt";
import type { BaseChatModel } from "@langchain/core/language_models/chat_models";
import type { StructuredToolInterface } from "@langchain/core/tools";

export function buildDefaultGraph(
    model: BaseChatModel,
    tools: StructuredToolInterface[],
) {
    const modelWithTools = model.bindTools(tools);
    const toolNode = new ToolNode(tools);

    async function callModel(state: typeof MessagesAnnotation.State) {
        const response = await modelWithTools.invoke(state.messages);
        return { messages: [response] };
    }

    function shouldContinue(state: typeof MessagesAnnotation.State) {
        const last = state.messages[state.messages.length - 1];
        return (last as { tool_calls?: unknown[] }).tool_calls?.length ? "tools" : "__end__";
    }

    return new StateGraph(MessagesAnnotation)
        .addNode("agent", callModel)
        .addNode("tools", toolNode)
        .addEdge("__start__", "agent")
        .addConditionalEdges("agent", shouldContinue)
        .addEdge("tools", "agent")
        .compile();
}
