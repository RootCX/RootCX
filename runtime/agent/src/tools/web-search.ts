import { tool } from "@langchain/core/tools";
import { z } from "zod";

export function createWebSearchTool() {
    return tool(
        async ({ query, count }) => {
            if (process.env.BRAVE_API_KEY) {
                return braveSearch(query, count ?? 5, process.env.BRAVE_API_KEY);
            }

            if (process.env.TAVILY_API_KEY) {
                return tavilySearch(query, count ?? 5, process.env.TAVILY_API_KEY);
            }

            return "Error: No search API key configured. Set BRAVE_API_KEY or TAVILY_API_KEY.";
        },
        {
            name: "web_search",
            description: "Search the internet for information. Returns a list of search results with titles, URLs, and snippets.",
            schema: z.object({
                query: z.string().describe("The search query"),
                count: z.number().optional().describe("Number of results to return (default: 5)"),
            }),
        },
    );
}

function formatResults(results: Array<{ title: string; url: string; snippet: string }>): string {
    return results
        .map((r, i) => `${i + 1}. ${r.title}\n   ${r.url}\n   ${r.snippet}`)
        .join("\n\n");
}

async function braveSearch(query: string, count: number, apiKey: string): Promise<string> {
    const url = new URL("https://api.search.brave.com/res/v1/web/search");
    url.searchParams.set("q", query);
    url.searchParams.set("count", String(count));

    const res = await fetch(url.toString(), {
        headers: { "Accept": "application/json", "X-Subscription-Token": apiKey },
    });
    if (!res.ok) return `Search error ${res.status}: ${await res.text()}`;

    const data = await res.json() as { web?: { results?: Array<{ title: string; url: string; description: string }> } };
    return formatResults((data.web?.results ?? []).map((r) => ({ title: r.title, url: r.url, snippet: r.description })));
}

async function tavilySearch(query: string, count: number, apiKey: string): Promise<string> {
    const res = await fetch("https://api.tavily.com/search", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ api_key: apiKey, query, max_results: count, search_depth: "basic" }),
    });
    if (!res.ok) return `Search error ${res.status}: ${await res.text()}`;

    const data = await res.json() as { results?: Array<{ title: string; url: string; content: string }> };
    return formatResults((data.results ?? []).map((r) => ({ title: r.title, url: r.url, snippet: r.content })));
}
