import { tool } from "@langchain/core/tools";
import { z } from "zod";

export function createMutateDataTool(
    appId: string,
    agentId: string,
    runtimeUrl: string,
) {
    return tool(
        async ({ entity, action, data, id }) => {
            const baseUrl = `${runtimeUrl}/api/v1/apps/${appId}/collections/${entity}`;
            const headers: Record<string, string> = {
                "Content-Type": "application/json",
                "X-Agent-Id": `agent:${agentId}`,
                "X-App-Id": appId,
            };

            let url = baseUrl;
            let method: string;

            switch (action) {
                case "create":
                    method = "POST";
                    break;
                case "update":
                    if (!id) return "Error: 'id' is required for update";
                    url = `${baseUrl}/${id}`;
                    method = "PATCH";
                    break;
                case "delete":
                    if (!id) return "Error: 'id' is required for delete";
                    url = `${baseUrl}/${id}`;
                    method = "DELETE";
                    break;
                default:
                    return `Error: unknown action '${action}'`;
            }

            const res = await fetch(url, {
                method,
                headers,
                body: action !== "delete" ? JSON.stringify(data ?? {}) : undefined,
            });

            if (!res.ok) {
                const text = await res.text();
                return `Error ${res.status}: ${text}`;
            }

            if (action === "delete") {
                return "Deleted successfully";
            }

            return JSON.stringify(await res.json());
        },
        {
            name: "mutate_data",
            description:
                "Create, update, or delete records in a data collection. Mutations are audit-logged.",
            schema: z.object({
                entity: z.string().describe("The collection/entity name (e.g. 'leads', 'research_notes')"),
                action: z.enum(["create", "update", "delete"]).describe("The mutation action"),
                data: z.record(z.string(), z.unknown()).optional().describe("The record data (for create/update)"),
                id: z.string().optional().describe("The record UUID (required for update/delete)"),
            }),
        },
    );
}
