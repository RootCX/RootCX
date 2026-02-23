import { tool } from "@langchain/core/tools";
import { z } from "zod";
import type { EntitySchema } from "../runner.js";
import { formatSchema } from "./schema.js";

export function createQueryDataTool(
    appId: string,
    agentId: string,
    runtimeUrl: string,
    authToken: string,
    dataContract: EntitySchema[],
) {
    return tool(
        async ({ entity, app, filter }) => {
            const targetApp = app ?? appId;
            const url = new URL(
                `/api/v1/apps/${targetApp}/collections/${entity}`,
                runtimeUrl,
            );

            if (filter) {
                for (const [key, value] of Object.entries(filter)) {
                    url.searchParams.set(key, String(value));
                }
            }

            const res = await fetch(url.toString(), {
                headers: {
                    "Authorization": `Bearer ${authToken}`,
                    "X-Agent-Id": `agent:${agentId}`,
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
                `Query records from a data collection.${formatSchema(dataContract)}`,
            schema: z.object({
                entity: z.string().describe("The collection/entity name"),
                app: z.string().optional().describe("Target app ID for cross-app reads. Omit to query own app."),
                filter: z.record(z.string(), z.unknown()).optional().describe("Key-value filter criteria"),
            }),
        },
    );
}
