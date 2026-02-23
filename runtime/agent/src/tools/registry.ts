import type { StructuredToolInterface } from "@langchain/core/tools";
import type { EntitySchema } from "../runner.js";
import { createQueryDataTool } from "./query-data.js";
import { createMutateDataTool } from "./mutate-data.js";
import { createWebSearchTool } from "./web-search.js";
import { createWebFetchTool } from "./web-fetch.js";

export interface ToolContext {
    appId: string;
    runtimeUrl: string;
    authToken: string;
    dataContract: EntitySchema[];
}

type ToolFactory = (ctx: ToolContext) => StructuredToolInterface;

const TOOL_FACTORIES: Record<string, ToolFactory> = {
    query_data: (ctx) => createQueryDataTool(ctx.appId, ctx.runtimeUrl, ctx.authToken, ctx.dataContract),
    mutate_data: (ctx) => createMutateDataTool(ctx.appId, ctx.runtimeUrl, ctx.authToken, ctx.dataContract),
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
