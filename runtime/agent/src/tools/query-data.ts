import { tool } from "@langchain/core/tools";
import { z } from "zod";

export function createQueryDataTool(
    appId: string,
    agentId: string,
    runtimeUrl: string,
) {
    return tool(
        async ({ entity, app, filter }) => {
            const targetApp = app ?? appId;
            const targetEntity = entity;
            const url = new URL(
                `/api/v1/apps/${targetApp}/collections/${targetEntity}`,
                runtimeUrl,
            );

            if (filter) {
                for (const [key, value] of Object.entries(filter)) {
                    url.searchParams.set(key, String(value));
                }
            }

            const res = await fetch(url.toString(), {
                headers: {
                    "X-Agent-Id": `agent:${agentId}`,
                    "X-App-Id": appId,
                },
            });

            if (!res.ok) {
                const text = await res.text();
                return `Error ${res.status}: ${text}`;
            }

            return JSON.stringify(await res.json());
        },
        {
            name: "query_data",
            description:
                "Query records from a data collection. Use this to read leads, research notes, or any entity the agent has access to.",
            schema: z.object({
                entity: z.string().describe("The collection/entity name to query (e.g. 'leads', 'research_notes')"),
                app: z.string().optional().describe("Target app ID for cross-app reads (e.g. 'crm'). Omit to query own app."),
                filter: z.record(z.string(), z.unknown()).optional().describe("Key-value filter criteria"),
            }),
        },
    );
}
