import { tool } from "@langchain/core/tools";
import { z } from "zod";

const MAX_LENGTH = 8000;

export function createWebFetchTool() {
    return tool(
        async ({ url, maxLength }) => {
            const limit = maxLength ?? MAX_LENGTH;
            try {
                const res = await fetch(url, {
                    headers: {
                        "User-Agent": "RootCX-Agent/1.0",
                        "Accept": "text/html,application/json,text/plain",
                    },
                    redirect: "follow",
                    signal: AbortSignal.timeout(15_000),
                });

                if (!res.ok) {
                    return `Fetch error ${res.status}: ${res.statusText}`;
                }

                const contentType = res.headers.get("content-type") ?? "";
                const text = await res.text();

                if (contentType.includes("text/html")) {
                    return truncate(htmlToText(text), limit);
                }

                return truncate(text, limit);
            } catch (err) {
                return `Fetch error: ${err instanceof Error ? err.message : String(err)}`;
            }
        },
        {
            name: "web_fetch",
            description:
                "Fetch a URL and return its readable text content. Useful for reading web pages, articles, or API responses.",
            schema: z.object({
                url: z.string().url().describe("The URL to fetch"),
                maxLength: z.number().optional().describe("Max characters to return (default: 8000)"),
            }),
        },
    );
}

// Intentionally simple regex-based stripping — adequate for extracting
// readable text from typical web pages. Not a full HTML parser.
function htmlToText(html: string): string {
    return html
        .replace(/<script[\s\S]*?<\/script>/gi, "")
        .replace(/<style[\s\S]*?<\/style>/gi, "")
        .replace(/<!--[\s\S]*?-->/g, "")
        .replace(/<\/(p|div|h[1-6]|li|tr|br|hr)[^>]*>/gi, "\n")
        .replace(/<br\s*\/?>/gi, "\n")
        .replace(/<[^>]+>/g, "")
        .replace(/&#(\d+);/g, (_, n) => String.fromCharCode(Number(n)))
        .replace(/&#x([0-9a-fA-F]+);/g, (_, h) => String.fromCharCode(parseInt(h, 16)))
        .replace(/&amp;/g, "&")
        .replace(/&lt;/g, "<")
        .replace(/&gt;/g, ">")
        .replace(/&quot;/g, '"')
        .replace(/&#39;/g, "'")
        .replace(/&nbsp;/g, " ")
        .replace(/[ \t]+/g, " ")
        .replace(/\n{3,}/g, "\n\n")
        .trim();
}

function truncate(text: string, maxLength: number): string {
    if (text.length <= maxLength) return text;
    return text.slice(0, maxLength) + "\n\n[... truncated]";
}
