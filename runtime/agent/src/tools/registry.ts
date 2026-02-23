import type { StructuredToolInterface } from "@langchain/core/tools";
import { createQueryDataTool } from "./query-data.js";
import { createMutateDataTool } from "./mutate-data.js";
import { createWebSearchTool } from "./web-search.js";
import { createWebFetchTool } from "./web-fetch.js";

export interface ToolContext {
    appId: string;
    agentId: string;
    runtimeUrl: string;
}

type ToolFactory = (ctx: ToolContext) => StructuredToolInterface;

const TOOL_FACTORIES: Record<string, ToolFactory> = {
    query_data: (ctx) => createQueryDataTool(ctx.appId, ctx.agentId, ctx.runtimeUrl),
    mutate_data: (ctx) => createMutateDataTool(ctx.appId, ctx.agentId, ctx.runtimeUrl),
    web_search: () => createWebSearchTool(),
    web_fetch: () => createWebFetchTool(),
};

export function buildToolRegistry(
    enabledTools: string[],
    ctx: ToolContext,
): StructuredToolInterface[] {
    return enabledTools
        .filter((name) => name in TOOL_FACTORIES)
        .map((name) => TOOL_FACTORIES[name](ctx));
}
