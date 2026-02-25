import { tool } from "@langchain/core/tools";
import { z } from "zod";
import type { ToolContext } from "../types.js";

const MAX_SNAPSHOT = 30_000;

function formatSnapshot(data: { text?: string; title?: string; url?: string }): string {
    const header = data.url ? `Page: ${data.title || "Untitled"} (${data.url})\n` : "";
    const body = data.text?.trim();
    if (!body) return `${header}[No content visible. Try scrolling or waiting for the page to load.]`;
    return header + body;
}

export function createTool({ runtimeUrl, authToken }: ToolContext) {
    const headers = { "Content-Type": "application/json", Authorization: `Bearer ${authToken}` };

    async function api(path: string, body?: Record<string, unknown>): Promise<Response> {
        return fetch(`${runtimeUrl}/api/v1/browser/${path}`, { method: "POST", headers, body: body ? JSON.stringify(body) : undefined });
    }

    async function snap(res: Response): Promise<string> {
        if (!res.ok) return `Error ${res.status}: ${await res.text()}`;
        const text = formatSnapshot(await res.json());
        return text.length <= MAX_SNAPSHOT ? text : text.slice(0, MAX_SNAPSHOT) + "\n\n[...truncated]";
    }

    async function ok(res: Response): Promise<string> {
        if (!res.ok) return `Error ${res.status}: ${await res.text()}`;
        const data = await res.json();
        return data.url ? `OK (${data.url})` : "OK";
    }

    return tool(
        async ({ action, url, ref_id, text, direction, amount, key, value, mode }) => {
            try {
                switch (action) {
                    case "navigate":
                        if (!url) return "Error: 'url' required";
                        return snap(await api("navigate", { url }));
                    case "click":
                        if (ref_id === undefined) return "Error: 'ref_id' required";
                        return ok(await api("click", { ref_id }));
                    case "type":
                        if (ref_id === undefined || !text) return "Error: 'ref_id' and 'text' required";
                        return ok(await api("type", { ref_id, text }));
                    case "scroll":
                        return ok(await api("scroll", { direction: direction ?? "down", amount: amount ?? 3 }));
                    case "press_key":
                        if (!key) return "Error: 'key' required";
                        return ok(await api("press_key", { key }));
                    case "select_option":
                        if (ref_id === undefined || !value) return "Error: 'ref_id' and 'value' required";
                        return ok(await api("select_option", { ref_id, value }));
                    case "hover":
                        if (ref_id === undefined) return "Error: 'ref_id' required";
                        return ok(await api("hover", { ref_id }));
                    case "snapshot":
                        return snap(await api("snapshot", { mode: mode ?? "full" }));
                    default:
                        return `Unknown action: ${action}`;
                }
            } catch (err) {
                return `Browser error: ${err instanceof Error ? err.message : String(err)}`;
            }
        },
        {
            name: "browser",
            description: `Control a real browser. Available actions:

navigate  — Go to URL. Returns page snapshot with element refs and readable text.
snapshot  — Get current page state. Use mode="efficient" for just interactive elements, or mode="full" (default) for full content.
click     — Click element by ref (e.g. ref_id: 5 for [e5]). Returns OK, not a snapshot.
type      — Type text into element. Clears existing value first. Returns OK.
press_key — Press Enter, Tab, Escape, ArrowDown, Space, etc. Returns OK.
select_option — Select dropdown option by value or visible text. Returns OK.
scroll    — Scroll the page. Returns OK.
hover     — Hover element to reveal tooltips/menus. Returns OK.

IMPORTANT: click/type/scroll/press_key/hover return OK without a snapshot. Call snapshot after to see the result.

Snapshot format:
  -- h1: "Dashboard" --
  Welcome to your dashboard
  -- navigation: "Main" --
    [e1] link "Home"
    [e2] link "Settings"
  [e3] textbox "Search" [focused]
  [e4] button "Submit"
  Error: please fill in all fields

Lines with [eN] are interactive — use their number as ref_id.
Lines without [eN] are readable text content.
States: [checked], [expanded], [collapsed], [disabled], [focused], [required]
[nth=N] disambiguates duplicate elements (e.g. two "Add to cart" buttons).`,
            schema: z.object({
                action: z.enum(["navigate", "click", "type", "scroll", "press_key", "select_option", "hover", "snapshot"]).describe("Browser action"),
                url: z.string().optional().describe("URL (for navigate)"),
                ref_id: z.number().int().optional().describe("Element ref number [eN] (for click/type/select_option/hover)"),
                text: z.string().optional().describe("Text to type (for type)"),
                direction: z.enum(["up", "down", "left", "right"]).optional().describe("Scroll direction"),
                amount: z.number().int().optional().describe("Scroll viewport units (default: 3)"),
                key: z.string().optional().describe("Key name (for press_key): Enter, Tab, Escape, ArrowDown, Space, etc."),
                value: z.string().optional().describe("Option value or text (for select_option)"),
                mode: z.enum(["full", "efficient"]).optional().describe("Snapshot mode (for snapshot): full=all content, efficient=interactive only"),
            }),
        },
    );
}
