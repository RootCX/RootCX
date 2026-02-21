import { DocsLayout } from "@/components/layout/DocsLayout";
import { PageNav } from "@/components/docs/PageNav";
import { CodeBlock } from "@/components/docs/CodeBlock";
import { Callout } from "@/components/docs/Callout";
import { ChevronRight } from "lucide-react";
import Link from "next/link";

const toc = [
    { id: "overview", title: "Overview" },
    { id: "configuration", title: "Configuration" },
    { id: "local-servers", title: "Local command servers" },
    { id: "remote-servers", title: "Remote HTTP servers" },
    { id: "use-cases", title: "What to build with MCP" },
];

export default function MCPPage() {
    return (
        <DocsLayout toc={toc}>
            <div className="flex flex-col gap-10">

                <div className="flex items-center gap-1.5 text-xs text-muted-foreground font-medium">
                    <Link href="/" className="hover:text-foreground transition-colors">RootCX</Link>
                    <ChevronRight className="h-3 w-3" />
                    <Link href="/ai" className="hover:text-foreground transition-colors">Building with AI</Link>
                    <ChevronRight className="h-3 w-3" />
                    <span className="text-foreground">MCP Servers</span>
                </div>

                <header className="flex flex-col gap-4" id="overview">
                    <h1 className="text-4xl font-semibold tracking-tight lg:text-5xl">MCP Servers</h1>
                    <p className="text-lg text-muted-foreground max-w-2xl leading-relaxed">
                        Extend what AI Forge can do by connecting it to external tools, live data sources, and internal services via the Model Context Protocol.
                    </p>
                </header>

                <section className="flex flex-col gap-4">
                    <p className="text-muted-foreground leading-7">
                        Out of the box, the Forge agents can read and write files in your project, run shell commands, and search your codebase. The <strong className="text-foreground font-medium">Model Context Protocol (MCP)</strong> is an open standard for exposing additional tools to an AI — any MCP server you connect becomes part of the agent's available toolset.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        OpenCode, the engine behind Forge, has native MCP support. You configure your MCP servers in <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">opencode.json</code> under the <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">mcp</code> key. Each entry is a named server — local (a subprocess over stdin/stdout) or remote (an HTTP endpoint).
                    </p>
                </section>

                <section className="flex flex-col gap-4" id="configuration">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Configuration</h2>
                    <p className="text-muted-foreground leading-7">
                        MCP servers are configured in <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">opencode.json</code> in your project root. The <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">mcp</code> key is a map of named server configs — the name is arbitrary and used for display in the Forge panel.
                    </p>
                    <CodeBlock
                        language="json"
                        filename="opencode.json"
                        code={`{
  "model": "anthropic/claude-sonnet-4-6",
  "mcp": {
    "my-local-tool": {
      "type": "local",
      "command": "bun",
      "args": ["run", "./tools/mcp-server.ts"]
    },
    "my-remote-tool": {
      "type": "remote",
      "url": "https://mcp.example.com/v1"
    }
  }
}`}
                    />
                    <Callout variant="info" title="Schema reference">
                        The full <code>McpLocalConfig</code> and <code>McpRemoteConfig</code> schema — including environment variables, headers, and OAuth options — is defined by OpenCode. Refer to the OpenCode documentation for the complete field reference.
                    </Callout>
                </section>

                <section className="flex flex-col gap-4" id="local-servers">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Local command servers</h2>
                    <p className="text-muted-foreground leading-7">
                        A local MCP server is a process that OpenCode spawns as a subprocess. Communication happens over stdin/stdout using the MCP JSON-RPC protocol. The subprocess is started when Forge connects to the project and killed when Studio closes.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        This is the simplest setup — no networking, no ports, no auth. Use it for tools that run on the same machine as Studio: querying your local PostgreSQL database, running project-specific scripts, wrapping CLI tools.
                    </p>
                    <Callout variant="tip" title="Querying the RootCX database">
                        The embedded PostgreSQL instance runs on port <code>5480</code> by default. A local MCP server can connect to it with a standard PostgreSQL driver and expose read-only query tools. This lets the AI fetch real sample data before generating transformation logic — instead of guessing at the shape of your rows.
                    </Callout>
                </section>

                <section className="flex flex-col gap-4" id="remote-servers">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Remote HTTP servers</h2>
                    <p className="text-muted-foreground leading-7">
                        A remote MCP server is an HTTP endpoint that implements the MCP protocol. This covers team-shared tools hosted on internal infrastructure, third-party MCP providers, and services that require persistent connections or OAuth flows.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        OpenCode handles the HTTP connection and, for servers that require it, the OAuth authorization flow — opening the browser, receiving the callback, and storing the token locally for subsequent connections.
                    </p>
                </section>

                <section className="flex flex-col gap-4" id="use-cases">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">What to build with MCP</h2>
                    <p className="text-muted-foreground leading-7">
                        MCP is worth setting up when the AI needs live data or external actions that it can't get from reading your project files. Some concrete cases for RootCX development:
                    </p>
                    <div className="flex flex-col gap-3">
                        {[
                            {
                                title: "Database inspection",
                                desc: "Expose a tool that runs a SELECT against the RootCX PostgreSQL instance and returns sample rows. Instead of writing worker logic that assumes what the data looks like, the AI can fetch 10 real rows and generate transformation code that handles your actual values, nulls, and edge cases.",
                            },
                            {
                                title: "Internal API access",
                                desc: "If you're building a worker that integrates with an internal REST API, expose that API as MCP tools. The AI can call it to understand the real response shape, then write the parsing and error-handling code around what it actually sees — not around what the docs say it returns.",
                            },
                            {
                                title: "Linting and code analysis",
                                desc: "Wrap your team's internal lint rules, ESLint config, or custom code analysis scripts as a local MCP tool. The build agent can invoke your linter mid-implementation, read the output, and fix issues before proposing the final diff.",
                            },
                            {
                                title: "Documentation and knowledge base",
                                desc: "Connect Forge to your team's internal wiki, Notion, or Confluence via a remote MCP server. When the AI needs to understand a business process — how your pricing tiers work, what counts as a billable event, what SLA means for your customers — it can query your documentation rather than asking you to re-explain it every session.",
                            },
                        ].map((u, i) => (
                            <div key={i} className="flex flex-col gap-1.5 rounded-xl border border-border bg-[#111] p-5">
                                <p className="font-semibold text-foreground text-sm">{u.title}</p>
                                <p className="text-sm text-muted-foreground leading-relaxed">{u.desc}</p>
                            </div>
                        ))}
                    </div>
                </section>

                <PageNav href="/ai/mcp" />
            </div>
        </DocsLayout>
    );
}
