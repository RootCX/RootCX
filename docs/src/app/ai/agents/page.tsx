import { DocsLayout } from "@/components/layout/DocsLayout";
import { PageNav } from "@/components/docs/PageNav";
import { Callout } from "@/components/docs/Callout";
import { ChevronRight } from "lucide-react";
import Link from "next/link";

const toc = [
    { id: "overview", title: "Overview" },
    { id: "explore", title: "explore" },
    { id: "plan", title: "plan" },
    { id: "build", title: "build" },
    { id: "general", title: "general" },
    { id: "how-they-collaborate", title: "How they collaborate" },
    { id: "per-agent-models", title: "Per-agent model config" },
];

export default function AgentsPage() {
    return (
        <DocsLayout toc={toc}>
            <div className="flex flex-col gap-10">

                <div className="flex items-center gap-1.5 text-xs text-muted-foreground font-medium">
                    <Link href="/" className="hover:text-foreground transition-colors">RootCX</Link>
                    <ChevronRight className="h-3 w-3" />
                    <Link href="/ai" className="hover:text-foreground transition-colors">Building with AI</Link>
                    <ChevronRight className="h-3 w-3" />
                    <span className="text-foreground">Agents</span>
                </div>

                <header className="flex flex-col gap-4" id="overview">
                    <h1 className="text-4xl font-semibold tracking-tight lg:text-5xl">Agents</h1>
                    <p className="text-lg text-muted-foreground max-w-2xl leading-relaxed">
                        OpenCode orchestrates four specialized agents — <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-sm text-foreground">explore</code>, <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-sm text-foreground">plan</code>, <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-sm text-foreground">build</code>, and <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-sm text-foreground">general</code> — each handling a distinct phase of the development loop.
                    </p>
                </header>

                <section className="flex flex-col gap-4">
                    <p className="text-muted-foreground leading-7">
                        Rather than one monolithic model that does everything, OpenCode composes a pipeline of agents where each agent has a focused role. You interact with the top-level session; the orchestrator decides which agent to invoke. Agent handoffs appear as <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">agent</code> parts in the Forge chat, bracketed by <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">step-start</code> and <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">step-finish</code> markers.
                    </p>
                </section>

                <section className="flex flex-col gap-4" id="explore">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">
                        <code className="font-mono">explore</code>
                    </h2>
                    <p className="text-muted-foreground leading-7">
                        The <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">explore</code> agent reads your project to build context before any code is written. It scans your manifest, existing workers, dependencies, and any instruction files. It does not modify files.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        Tool calls from the explore agent appear in the chat as <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">tool</code> parts with read operations. Each requires your approval via a permission card. Because explore is read-only, approving these is low-risk — you can use <strong className="text-foreground font-medium">Always Allow</strong> for file reads without concern.
                    </p>
                    <Callout variant="tip" title="What explore reads">
                        In a RootCX project, explore will typically read <code>manifest.json</code> (to understand your data model and RBAC), <code>worker/index.ts</code> and supporting worker files (to understand existing logic), <code>package.json</code> (to know what dependencies are available), and any <code>AGENTS.md</code> or instruction files you have configured. It uses this to ensure the plan and build agents produce output that fits.
                    </Callout>
                </section>

                <section className="flex flex-col gap-4" id="plan">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">
                        <code className="font-mono">plan</code>
                    </h2>
                    <p className="text-muted-foreground leading-7">
                        The <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">plan</code> agent takes the output of explore and produces a numbered implementation plan. It does not write files — it writes a description of what needs to change and why.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        The plan is presented as text in the chat. You can reply directly to push back on any step: "skip step 3, I'll handle the migration manually" or "add error handling for missing foreign keys". The plan agent will revise. Approve the plan (or just reply "proceed") to hand off to the build agent.
                    </p>
                    <Callout variant="tip" title="Invest time here">
                        The plan is the cheapest point to change direction. A one-sentence correction to the plan avoids minutes of reverting generated code. Describe schema constraints, access rules, and edge cases here — the build agent will follow them precisely.
                    </Callout>
                </section>

                <section className="flex flex-col gap-4" id="build">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">
                        <code className="font-mono">build</code>
                    </h2>
                    <p className="text-muted-foreground leading-7">
                        The <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">build</code> agent implements the plan. It proposes file writes and edits one at a time, emitting permission cards for each. You review the diff and approve, approve-always, or deny before the next step.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        The build agent may also invoke shell commands — for example, running a type-check after writing TypeScript to verify correctness. These also require your approval. If a command fails, the agent can read the error output and self-correct before proposing the next change.
                    </p>
                    <Callout variant="warning" title="Review manifest changes carefully">
                        Changes to <code>manifest.json</code> proposed by the build agent will affect your PostgreSQL schema when you install the app from Studio. Review them with the same care you'd give a database migration — because they are one.
                    </Callout>
                </section>

                <section className="flex flex-col gap-4" id="general">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">
                        <code className="font-mono">general</code>
                    </h2>
                    <p className="text-muted-foreground leading-7">
                        The <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">general</code> agent handles questions, explanations, and discussions that don't require reading your codebase or writing files. It's the fastest agent — no tool calls, no file access, just the LLM responding directly.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        Use it when you want a conversation: explaining what a piece of code does, discussing architecture trade-offs, reviewing a design before committing to it, or getting a second opinion on an error message. If you paste a stack trace and ask what's wrong, the general agent answers without needing to explore your project.
                    </p>
                </section>

                <section className="flex flex-col gap-4" id="how-they-collaborate">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">How they collaborate</h2>
                    <p className="text-muted-foreground leading-7">
                        For a typical feature request — "add a Stripe webhook handler that updates subscription status" — the orchestrator runs something like this:
                    </p>
                    <div className="flex flex-col gap-0 rounded-xl border border-border overflow-hidden">
                        {[
                            { agent: "explore", what: "Reads manifest.json, worker/index.ts, package.json. Confirms stripe is not yet in dependencies. Finds the subscriptions table." },
                            { agent: "plan", what: "Outlines: (1) add stripe to package.json, (2) add a POST /webhooks/stripe RPC handler, (3) verify Stripe signature, (4) handle customer.subscription.updated and invoice.paid events, (5) update the subscriptions and invoices tables." },
                            { agent: "general", what: "Asks a clarifying question if needed: \"Should the webhook route be authenticated or use Stripe signature verification only?\"" },
                            { agent: "build", what: "Writes the code file by file. Runs bun typecheck after each file. Proposes diffs; each requires your approval." },
                        ].map((s, i) => (
                            <div key={i} className="flex items-start gap-4 px-5 py-4 border-b border-border last:border-0">
                                <code className="shrink-0 font-mono text-xs font-semibold text-primary bg-primary/10 rounded px-2 py-0.5 mt-0.5">{s.agent}</code>
                                <p className="text-sm text-muted-foreground leading-relaxed">{s.what}</p>
                            </div>
                        ))}
                    </div>
                    <p className="text-muted-foreground leading-7">
                        You can see every handoff in the chat as named <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">agent</code> parts. The entire execution is auditable — every tool call, every step boundary, every permission request is visible in the Forge panel.
                    </p>
                </section>

                <section className="flex flex-col gap-4" id="per-agent-models">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Per-agent model config</h2>
                    <p className="text-muted-foreground leading-7">
                        You can configure a different model for each agent in <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">opencode.json</code>. This lets you use a fast, cheap model for read-heavy explore steps and a more capable model where it matters — in the build agent that writes the actual code.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        The top-level <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">model</code> field is the default. Agent-level overrides in the <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">agent</code> block take precedence for that specific agent.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        Refer to the <Link href="/ai/providers" className="text-foreground underline underline-offset-4 hover:text-primary transition-colors">LLM Providers</Link> page for the full config schema including per-agent model overrides.
                    </p>
                </section>

                <PageNav href="/ai/agents" />
            </div>
        </DocsLayout>
    );
}
