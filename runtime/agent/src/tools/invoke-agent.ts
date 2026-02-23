import { tool } from "@langchain/core/tools";
import { z } from "zod";

export function createInvokeAgentTool(
    appId: string,
    agentId: string,
    runtimeUrl: string,
    authToken: string,
) {
    return tool(
        async ({ agent_id, message, app_id }) => {
            const targetApp = app_id ?? appId;
            const url = `${runtimeUrl}/api/v1/apps/${targetApp}/agents/${agent_id}/invoke`;

            const res = await fetch(url, {
                method: "POST",
                headers: {
                    "Content-Type": "application/json",
                    "Authorization": `Bearer ${authToken}`,
                    "X-Agent-Id": `agent:${agentId}`,
                },
                body: JSON.stringify({ message }),
            });

            if (!res.ok) {
                return `Error ${res.status}: ${await res.text()}`;
            }

            // Collect SSE stream into final response
            const text = await res.text();
            let response = "";
            for (const line of text.split("\n")) {
                if (!line.startsWith("data: ")) continue;
                try {
                    const data = JSON.parse(line.slice(6));
                    if (data.error) return `Agent error: ${data.error}`;
                    if (data.response) response = data.response;
                    else if (data.delta) response += data.delta;
                } catch { /* non-JSON line */ }
            }
            return response || "No response from agent";
        },
        {
            name: "invoke_agent",
            description:
                "Invoke another AI agent and get its response. Use this to delegate tasks to specialized agents.",
            schema: z.object({
                agent_id: z.string().describe("The ID of the agent to invoke"),
                message: z.string().describe("The message/task to send to the agent"),
                app_id: z.string().optional().describe("Target app ID (omit to invoke agent in same app)"),
            }),
        },
    );
}
