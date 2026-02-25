import { DynamicStructuredTool } from "@langchain/core/tools";
import { z } from "zod";

export interface ToolDescriptor {
    name: string;
    description: string;
    inputSchema: Record<string, unknown>;
}

export function buildTools(
    descriptors: ToolDescriptor[],
    appId: string,
    runtimeUrl: string,
    authToken: string,
): DynamicStructuredTool[] {
    const headers = { "Content-Type": "application/json", Authorization: `Bearer ${authToken}` };
    return descriptors.map((d) => new DynamicStructuredTool({
        name: d.name,
        description: d.description,
        schema: toZod(d.inputSchema) as z.ZodObject<any>,
        func: async (args: Record<string, unknown>) => {
            try {
                const res = await fetch(`${runtimeUrl}/api/v1/tools/${d.name}/execute`, {
                    method: "POST", headers, body: JSON.stringify({ appId, args }),
                });
                if (!res.ok) return `Error ${res.status}: ${await res.text()}`;
                return JSON.stringify(await res.json());
            } catch (err) {
                return `Tool error: ${err instanceof Error ? err.message : String(err)}`;
            }
        },
    }));
}

function toZod(s: Record<string, unknown>): z.ZodTypeAny {
    if (Array.isArray(s.enum)) return z.enum(s.enum as [string, ...string[]]);
    const desc = (s.description as string) ?? "";
    switch (s.type as string) {
        case "string": return z.string().describe(desc);
        case "boolean": return z.boolean().describe(desc);
        case "number": case "integer": {
            let n = s.type === "integer" ? z.number().int() : z.number();
            if (typeof s.minimum === "number") n = n.min(s.minimum);
            if (typeof s.maximum === "number") n = n.max(s.maximum);
            return n.describe(desc);
        }
        case "array": return z.array(s.items ? toZod(s.items as Record<string, unknown>) : z.unknown()).describe(desc);
        case "object": {
            const props = s.properties as Record<string, Record<string, unknown>> | undefined;
            if (!props) return z.record(z.string(), z.unknown()).describe(desc);
            const req = new Set((s.required as string[]) ?? []);
            const shape: Record<string, z.ZodTypeAny> = {};
            for (const [k, v] of Object.entries(props)) {
                shape[k] = req.has(k) ? toZod(v) : toZod(v).optional();
            }
            return z.object(shape).describe(desc);
        }
        default: return z.unknown().describe(desc);
    }
}
