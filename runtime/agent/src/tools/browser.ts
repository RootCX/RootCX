import { tool } from "@langchain/core/tools";
import { z } from "zod";

const MAX_SNAPSHOT_LENGTH = 8000;

function formatSnapshot(data: { text?: string; title?: string; url?: string }): string {
    const header = data.url ? `Page: ${data.title || "Untitled"} (${data.url})\n` : "";
    const body = data.text?.trim();
    if (!body) return `${header}[Page has no interactive elements visible. The page may require login, may still be loading, or may be empty.]`;
    return header + body;
}

export function createBrowserTool(runtimeUrl: string, authToken: string) {
    const headers = {
        "Content-Type": "application/json",
        Authorization: `Bearer ${authToken}`,
    };

    async function browserApi(path: string, body?: Record<string, unknown>): Promise<Response> {
        return fetch(`${runtimeUrl}/api/v1/browser/${path}`, {
            method: "POST",
            headers,
            body: body ? JSON.stringify(body) : undefined,
        });
    }

    async function snap(res: Response): Promise<string> {
        if (!res.ok) return `Error ${res.status}: ${await res.text()}`;
        const text = formatSnapshot(await res.json());
        return text.length <= MAX_SNAPSHOT_LENGTH ? text : text.slice(0, MAX_SNAPSHOT_LENGTH) + "\n\n[... truncated]";
    }

    return tool(
        async ({ action, url, ref_id, text, direction, amount }) => {
            try {
                switch (action) {
                    case "navigate": {
                        if (!url) return "Error: 'url' is required for navigate";
                        return snap(await browserApi("navigate", { url }));
                    }
                    case "click": {
                        if (ref_id === undefined) return "Error: 'ref_id' is required for click";
                        return snap(await browserApi("click", { ref_id }));
                    }
                    case "type": {
                        if (ref_id === undefined) return "Error: 'ref_id' is required for type";
                        if (!text) return "Error: 'text' is required for type";
                        return snap(await browserApi("type", { ref_id, text }));
                    }
                    case "scroll":
                        return snap(await browserApi("scroll", {
                            direction: direction ?? "down",
                            amount: amount ?? 3,
                        }));
                    case "snapshot":
                        return snap(await browserApi("snapshot"));
                    default:
                        return `Unknown action: ${action}`;
                }
            } catch (err) {
                return `Browser error: ${err instanceof Error ? err.message : String(err)}`;
            }
        },
        {
            name: "browser",
            description: `Browse the web interactively. Actions:
- navigate: Go to a URL. Returns page snapshot with numbered refs.
- click: Click element by ref number. Returns updated snapshot.
- type: Type into element with keystrokes (works with autocomplete/search). Returns updated snapshot.
- scroll: Scroll the page. Returns updated snapshot.
- snapshot: Get current page text with clickable refs.

After navigate/click/type/scroll, you receive a text snapshot with numbered refs like:
[e1] link "Home" [e2] button "Submit" [e3] textbox "Search"
Use ref numbers for click/type actions.`,
            schema: z.object({
                action: z
                    .enum(["navigate", "click", "type", "scroll", "snapshot"])
                    .describe("The browser action to perform"),
                url: z.string().optional().describe("URL to navigate to (for 'navigate')"),
                ref_id: z.number().int().optional().describe("Element ref number from snapshot (for 'click'/'type')"),
                text: z.string().optional().describe("Text to type (for 'type')"),
                direction: z.enum(["up", "down", "left", "right"]).optional().describe("Scroll direction (default: down)"),
                amount: z.number().int().optional().describe("Scroll amount in viewport units (default: 3)"),
            }),
        },
    );
}
