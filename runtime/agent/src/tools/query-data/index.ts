import { tool } from "@langchain/core/tools";
import { z } from "zod";
import { formatSchema } from "../schema.js";
import type { ToolContext } from "../types.js";

const WHERE_DSL = `\
WHERE DSL (MongoDB-style):
- Equality shorthand: {"field":"value"}
- Operators: {"field":{"$op":value}} — $eq $ne $gt $gte $lt $lte $like $ilike $in $contains $isNull
- $like/$ilike: SQL pattern (% = wildcard). $in: array. $contains: array subset. $isNull: bool.
- Logic: $and:[...] $or:[...] $not:{...} — nestable. Top-level keys are AND-ed.
Example: {"$or":[{"status":"active"},{"role":"admin"}],"age":{"$gte":18}}`;

export function createTool({ appId, runtimeUrl, authToken, dataContract }: ToolContext) {
    const headers = { Authorization: `Bearer ${authToken}` };

    return tool(
        async ({ entity, app, where: w, orderBy, order, limit, offset }) => {
            const base = `${runtimeUrl}/api/v1/apps/${app ?? appId}/collections/${entity}`;
            const useQuery = w || orderBy || order || limit || offset;

            const res = useQuery
                ? await fetch(`${base}/query`, {
                      method: "POST",
                      headers: { ...headers, "Content-Type": "application/json" },
                      body: JSON.stringify({ where: w, orderBy, order, limit, offset }),
                  })
                : await fetch(base, { headers });

            if (!res.ok) return `Error ${res.status}: ${await res.text()}`;
            return JSON.stringify(await res.json());
        },
        {
            name: "query_data",
            description: `Query records from a collection. Returns {data,total} with filters, or T[] for simple list.\n${WHERE_DSL}${formatSchema(dataContract)}`,
            schema: z.object({
                entity: z.string().describe("Collection/entity name"),
                app: z.string().optional().describe("Target app ID for cross-app reads"),
                where: z.record(z.string(), z.unknown()).optional().describe("WHERE clause — see DSL above"),
                orderBy: z.string().optional().describe("Sort field (default: created_at)"),
                order: z.enum(["asc", "desc"]).optional().describe("Sort direction (default: desc)"),
                limit: z.number().int().min(1).max(1000).optional().describe("Max rows (default: 100)"),
                offset: z.number().int().min(0).optional().describe("Skip N rows (default: 0)"),
            }),
        },
    );
}
