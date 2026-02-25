import { readdirSync, readFileSync } from "fs";
import { join, dirname } from "path";
import { fileURLToPath } from "url";
import type { StructuredToolInterface } from "@langchain/core/tools";
import type { ToolContext } from "./types.js";

export type { ToolContext };

const __dirname = dirname(fileURLToPath(import.meta.url));

const discovered = new Map<string, { dirName: string; factory?: (ctx: ToolContext) => StructuredToolInterface }>();

for (const entry of readdirSync(__dirname, { withFileTypes: true })) {
    if (!entry.isDirectory()) continue;
    try {
        const { name } = JSON.parse(readFileSync(join(__dirname, entry.name, "meta.json"), "utf-8"));
        discovered.set(name, { dirName: entry.name });
    } catch {}
}

export async function buildToolRegistry(
    enabledTools: string[],
    ctx: ToolContext,
): Promise<StructuredToolInterface[]> {
    const tools: StructuredToolInterface[] = [];
    for (const name of enabledTools) {
        const entry = discovered.get(name);
        if (!entry) {
            console.warn(`Unknown tool "${name}", skipping`);
            continue;
        }
        if (!entry.factory) {
            const mod = await import(`./${entry.dirName}/index.js`);
            entry.factory = mod.createTool;
        }
        tools.push(entry.factory!(ctx));
    }
    return tools;
}
