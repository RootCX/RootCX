import { DocsLayout } from "@/components/layout/DocsLayout";
import { PageNav } from "@/components/docs/PageNav";
import { CodeBlock } from "@/components/docs/CodeBlock";
import { Callout } from "@/components/docs/Callout";
import { ChevronRight } from "lucide-react";
import Link from "next/link";

const toc = [
    { id: "overview", title: "Overview" },
    { id: "how-it-works", title: "How it works" },
    { id: "chat-interface", title: "Chat interface" },
    { id: "message-parts", title: "Message parts" },
    { id: "permissions", title: "Tool permissions" },
    { id: "questions", title: "Interactive questions" },
    { id: "sessions", title: "Sessions" },
    { id: "configuration", title: "Configuration" },
];

export default function ForgePage() {
    return (
        <DocsLayout toc={toc}>
            <div className="flex flex-col gap-10">

                <div className="flex items-center gap-1.5 text-xs text-muted-foreground font-medium">
                    <Link href="/" className="hover:text-foreground transition-colors">RootCX</Link>
                    <ChevronRight className="h-3 w-3" />
                    <Link href="/studio" className="hover:text-foreground transition-colors">Studio IDE</Link>
                    <ChevronRight className="h-3 w-3" />
                    <span className="text-foreground">AI Forge</span>
                </div>

                <header className="flex flex-col gap-4" id="overview">
                    <h1 className="text-4xl font-semibold tracking-tight lg:text-5xl">AI Forge</h1>
                    <p className="text-lg text-muted-foreground max-w-2xl leading-relaxed">
                        An AI coding assistant embedded in Studio, powered by OpenCode. Connect any LLM provider and build internal software through conversation.
                    </p>
                </header>

                <section className="flex flex-col gap-4">
                    <p className="text-muted-foreground leading-7">
                        <strong className="text-foreground font-medium">AI Forge</strong> is the panel in Studio that connects to <strong className="text-foreground font-medium">OpenCode</strong> — an open-source AI coding engine that runs as a local sidecar process. OpenCode manages LLM provider connectivity, agent orchestration, tool execution, and session persistence. Forge surfaces all of this through a chat panel integrated into Studio.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        Your code never leaves your machine. Only the messages you explicitly send reach your chosen LLM provider.
                    </p>
                </section>

                <section className="flex flex-col gap-4" id="how-it-works">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">How it works</h2>
                    <p className="text-muted-foreground leading-7">
                        When you open a project in Studio, Tauri invokes <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">start_forge</code>, which spawns the <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">opencode serve</code> binary on a random local port. Studio then calls <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">get_forge_status</code> to discover which port was assigned, and the Forge panel connects to <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">http://127.0.0.1:{"{port}"}</code> using the <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">@opencode-ai/sdk</code> client.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        All communication between the Forge panel and OpenCode happens over a <strong className="text-foreground font-medium">server-sent events (SSE) stream</strong>. Message parts, tool calls, permission requests, and session updates all arrive through this stream in real time.
                    </p>
                    <Callout variant="info" title="Health check">
                        Studio polls <code>{"http://127.0.0.1:{port}/global/health"}</code> after spawning the OpenCode process. The panel shows a red dot until that endpoint responds — typically within a few hundred milliseconds.
                    </Callout>
                </section>

                <section className="flex flex-col gap-4" id="chat-interface">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Chat interface</h2>
                    <p className="text-muted-foreground leading-7">
                        The input box at the bottom of the Forge panel accepts your prompt. Press <kbd className="rounded border border-border bg-[#111] px-1.5 py-0.5 font-mono text-xs">Enter</kbd> to send, <kbd className="rounded border border-border bg-[#111] px-1.5 py-0.5 font-mono text-xs">Shift+Enter</kbd> for a new line. The <kbd className="rounded border border-border bg-[#111] px-1.5 py-0.5 font-mono text-xs">■</kbd> stop button aborts the current generation mid-stream.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        The input is disabled while the model is generating (<span className="font-mono text-xs text-foreground">Thinking...</span>) and while Studio is connecting to OpenCode (<span className="font-mono text-xs text-foreground">Connecting...</span>).
                    </p>
                </section>

                <section className="flex flex-col gap-4" id="message-parts">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Message parts</h2>
                    <p className="text-muted-foreground leading-7">
                        Each message is composed of typed <strong className="text-foreground font-medium">parts</strong> that stream in progressively. The panel renders each part type differently:
                    </p>
                    <div className="flex flex-col gap-2">
                        {[
                            { type: "text", desc: "Plain prose from the model — explanations, answers, code reviews." },
                            { type: "reasoning", desc: "Visible chain-of-thought from reasoning-capable models. Rendered in italics, muted." },
                            { type: "tool", desc: "A tool the AI invoked. Shows the tool name, a live status badge (running / done / err), and a title when available." },
                            { type: "agent", desc: "Marks a handoff to a named sub-agent (plan, build, general, explore)." },
                            { type: "step-start / step-finish", desc: "Boundaries of a multi-step agent invocation." },
                            { type: "compaction", desc: "Indicates the conversation history was summarized to fit within the model's context window." },
                        ].map((p, i) => (
                            <div key={i} className="flex items-start gap-3 rounded-lg border border-border bg-[#111] px-4 py-3">
                                <code className="shrink-0 rounded bg-primary/10 px-2 py-0.5 font-mono text-xs text-primary">{p.type}</code>
                                <p className="text-sm text-muted-foreground leading-relaxed">{p.desc}</p>
                            </div>
                        ))}
                    </div>
                </section>

                <section className="flex flex-col gap-4" id="permissions">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Tool permissions</h2>
                    <p className="text-muted-foreground leading-7">
                        When OpenCode wants to execute a tool — write a file, run a command, make an HTTP call — it pauses and emits a <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">permission.updated</code> event. A permission card appears in the chat with the tool name, type, and pattern. You choose:
                    </p>
                    <div className="flex flex-col gap-2">
                        {[
                            { label: "Allow Once", value: "once", desc: "Approve this single action. The AI will ask again next time." },
                            { label: "Always Allow", value: "always", desc: "Grant blanket approval for this tool for the rest of the session." },
                            { label: "Deny", value: "reject", desc: "Block this action. The AI receives a rejection and can propose an alternative." },
                        ].map((p, i) => (
                            <div key={i} className="flex items-start gap-3 rounded-lg border border-border bg-[#111] px-4 py-3">
                                <div className="shrink-0 flex flex-col gap-0.5">
                                    <span className="font-semibold text-foreground text-sm">{p.label}</span>
                                    <code className="font-mono text-[10px] text-muted-foreground/60">{p.value}</code>
                                </div>
                                <p className="text-sm text-muted-foreground leading-relaxed">{p.desc}</p>
                            </div>
                        ))}
                    </div>
                    <Callout variant="warning" title="Always Allow is session-scoped">
                        The <code>always</code> grant does not persist across sessions. Each new session starts with a clean permission state.
                    </Callout>
                </section>

                <section className="flex flex-col gap-4" id="questions">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Interactive questions</h2>
                    <p className="text-muted-foreground leading-7">
                        Some tool calls and agent steps emit <strong className="text-foreground font-medium">question cards</strong> — structured prompts that ask you to pick from predefined options before the AI continues. Questions can be single-select or multi-select, with an optional free-text custom answer field.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        Once all fields are filled, hit <strong className="text-foreground font-medium">Submit</strong>. If you want to skip the question and let the AI proceed without your input, hit <strong className="text-foreground font-medium">Skip</strong>.
                    </p>
                </section>

                <section className="flex flex-col gap-4" id="sessions">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Sessions</h2>
                    <p className="text-muted-foreground leading-7">
                        Forge organizes conversations into <strong className="text-foreground font-medium">sessions</strong>. The session selector at the top of the panel shows a dropdown of all sessions for the current project. Click <strong className="text-foreground font-medium">New</strong> to start a fresh one. Sessions persist across Studio restarts — all messages, tool calls, and parts are stored locally by OpenCode.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        Switching sessions loads the full message and part history for that session. Sessions are tied to the project directory — different projects have independent histories.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        See <Link href="/ai/sessions" className="text-foreground underline underline-offset-4 hover:text-primary transition-colors">Sessions & Commands</Link> for details on commands and context management.
                    </p>
                </section>

                <section className="flex flex-col gap-4" id="configuration">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Configuration</h2>
                    <p className="text-muted-foreground leading-7">
                        The gear icon in the Forge panel header opens the settings panel. Select a provider and model from the dropdowns — providers marked <span className="font-mono text-xs text-foreground">(connected)</span> have valid credentials. Enter an API key directly, or set it as an environment variable (the required env var name is shown below the input for each provider).
                    </p>
                    <p className="text-muted-foreground leading-7">
                        Settings are saved to <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">opencode.json</code> in the project root via the <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">save_forge_config</code> Tauri command. The config format:
                    </p>
                    <CodeBlock
                        language="json"
                        filename="opencode.json"
                        code={`{
  "model": "anthropic/claude-sonnet-4-6",
  "provider": {
    "anthropic": {
      "options": { "apiKey": "sk-ant-..." }
    }
  }
}`}
                    />
                    <Callout variant="tip" title="Environment variables">
                        API keys can be set as env vars instead of hardcoded in <code>opencode.json</code>. For Anthropic, set <code>ANTHROPIC_API_KEY</code>. The provider dropdown shows the required env var name for each provider — set it in your shell profile and leave the API Key field blank.
                    </Callout>
                    <p className="text-muted-foreground leading-7">
                        For advanced configuration — custom commands, MCP servers, per-agent model overrides, LSP, formatters — edit <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">opencode.json</code> directly. See <Link href="/ai/providers" className="text-foreground underline underline-offset-4 hover:text-primary transition-colors">LLM Providers</Link> and <Link href="/ai/mcp" className="text-foreground underline underline-offset-4 hover:text-primary transition-colors">MCP Servers</Link> for reference.
                    </p>
                </section>

                <PageNav href="/studio/forge" />
            </div>
        </DocsLayout>
    );
}
