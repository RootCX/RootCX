import { DocsLayout } from "@/components/layout/DocsLayout";
import { PageNav } from "@/components/docs/PageNav";
import { CodeBlock } from "@/components/docs/CodeBlock";
import { Callout } from "@/components/docs/Callout";
import { ChevronRight } from "lucide-react";
import Link from "next/link";

const toc = [
    { id: "sessions", title: "Sessions" },
    { id: "context-compaction", title: "Context compaction" },
    { id: "commands", title: "Custom commands" },
    { id: "instructions", title: "Instruction files" },
];

export default function SessionsPage() {
    return (
        <DocsLayout toc={toc}>
            <div className="flex flex-col gap-10">

                <div className="flex items-center gap-1.5 text-xs text-muted-foreground font-medium">
                    <Link href="/" className="hover:text-foreground transition-colors">RootCX</Link>
                    <ChevronRight className="h-3 w-3" />
                    <Link href="/ai" className="hover:text-foreground transition-colors">Building with AI</Link>
                    <ChevronRight className="h-3 w-3" />
                    <span className="text-foreground">Sessions & Commands</span>
                </div>

                <header className="flex flex-col gap-4" id="sessions">
                    <h1 className="text-4xl font-semibold tracking-tight lg:text-5xl">Sessions & Commands</h1>
                    <p className="text-lg text-muted-foreground max-w-2xl leading-relaxed">
                        Sessions are persistent conversation threads. Commands are reusable prompt templates. Instructions are Markdown files that give the AI standing context about your project.
                    </p>
                </header>

                <section className="flex flex-col gap-4">
                    <p className="text-muted-foreground leading-7">
                        A <strong className="text-foreground font-medium">session</strong> is a persistent conversation between you and Forge. OpenCode stores the full message history — every prompt, every agent step, every tool call and its result — locally on your machine, tied to the current project directory. Close Studio and reopen it; your session is exactly where you left it.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        The session selector at the top of the Forge panel shows the current session and a dropdown of all sessions for this project. Click <strong className="text-foreground font-medium">New</strong> to start a blank session. Sessions are created automatically when you send your first message to a project with no existing session.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        Sessions are title by default from their first message. Keep sessions scoped to one task or feature — mixing unrelated work in a single session dilutes the context and leads to confused output.
                    </p>
                </section>

                <section className="flex flex-col gap-4" id="context-compaction">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Context compaction</h2>
                    <p className="text-muted-foreground leading-7">
                        Every LLM has a context window limit. Long sessions accumulate history that eventually exceeds it. When this happens, OpenCode automatically compacts the conversation — summarizing the older portion and keeping the summary in the prompt. A <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">compaction</code> part appears in the Forge chat at the point where summarization occurred.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        The full session history is always available in the Forge panel — compaction only affects what's sent to the LLM in each request, not what you can read. The <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">auto</code> field on the compaction part indicates whether it was triggered automatically or manually.
                    </p>
                    <Callout variant="tip" title="When to start a new session">
                        Compaction works well within a focused task. When you move to a completely different feature, start a new session rather than continuing in a compacted one. A fresh session with a clean slate produces better output than one where the AI is reasoning from a lossy summary of unrelated prior work.
                    </Callout>
                </section>

                <section className="flex flex-col gap-4" id="commands">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Custom commands</h2>
                    <p className="text-muted-foreground leading-7">
                        <strong className="text-foreground font-medium">Commands</strong> are reusable prompt templates defined in <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">opencode.json</code> under the <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">command</code> key. Each command has a template string plus optional metadata — a description, a specific agent to route it to, and a model override.
                    </p>
                    <CodeBlock
                        language="json"
                        filename="opencode.json — command definitions"
                        code={`{
  "model": "anthropic/claude-sonnet-4-6",
  "command": {
    "new-table": {
      "description": "Scaffold a new table in the manifest",
      "agent": "build",
      "template": "Add a new table to manifest.json with the following spec: {spec}. Include standard columns (id UUID primary key, created_at, updated_at). Add RBAC rules: {rbac}."
    },
    "review-manifest": {
      "description": "Review manifest.json for issues",
      "agent": "general",
      "template": "Review manifest.json. Check for: missing indexes on foreign keys, overly permissive RBAC rules, enum columns that should be lookup tables, and column naming inconsistencies. Return a prioritized list of issues."
    },
    "worker-skeleton": {
      "description": "Generate a typed worker skeleton",
      "agent": "build",
      "template": "Generate a worker/index.ts skeleton for the following RPCs and jobs: {spec}. Include TypeScript types, input validation, and JSDoc. Use process.env for all secrets."
    }
  }
}`}
                    />
                    <p className="text-muted-foreground leading-7">
                        The <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">template</code> is a plain string — use whatever placeholder convention you like (e.g., <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">{"{spec}"}</code>, <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">{"{rbac}"}</code>). The command expands in the input field when selected and you fill in the placeholders before sending.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        The <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">subtask</code> boolean (optional) runs the command as a background subtask rather than as a message in the main session thread — useful for longer analysis jobs that shouldn't clutter the main conversation.
                    </p>
                </section>

                <section className="flex flex-col gap-4" id="instructions">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Instruction files</h2>
                    <p className="text-muted-foreground leading-7">
                        <strong className="text-foreground font-medium">Instruction files</strong> are Markdown documents that OpenCode reads automatically before every session — they give the AI standing context about your project without you having to repeat yourself.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        Instruction files are resolved by Tauri via the <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">resolve_instructions</code> command. The resolved files are listed in the Forge Settings panel under <strong className="text-foreground font-medium">Instructions</strong>. A warning is shown if no instruction files are found.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        You configure which files to include in <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">opencode.json</code> under the <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">instructions</code> key — an array of file paths relative to the project root.
                    </p>
                    <CodeBlock
                        language="json"
                        filename="opencode.json — instruction files"
                        code={`{
  "model": "anthropic/claude-sonnet-4-6",
  "instructions": [
    "AGENTS.md",
    "docs/architecture.md"
  ]
}`}
                    />
                    <p className="text-muted-foreground leading-7">
                        What to put in an instruction file:
                    </p>
                    <div className="flex flex-col gap-2">
                        {[
                            { heading: "Project overview", desc: "What the application does, who uses it, what the core data entities are." },
                            { heading: "Conventions", desc: "Column naming rules (snake_case, no abbreviations), error handling patterns, how you structure workers, which npm packages are preferred." },
                            { heading: "RBAC patterns", desc: "What roles exist, what the ownership model is (e.g., \"created_by is always a FK to users and controls row-level access\")." },
                            { heading: "Off-limits areas", desc: "Files or patterns the AI should not touch — for example, \"do not modify the migrations/ directory, those are managed manually\"." },
                            { heading: "Secrets reference", desc: "The names of secrets in the vault (not their values), so the AI knows what's available when writing worker code that needs credentials." },
                        ].map((item, i) => (
                            <div key={i} className="flex items-start gap-3 rounded-lg border border-border bg-[#111] px-4 py-3">
                                <span className="shrink-0 font-semibold text-foreground text-sm">{item.heading}</span>
                                <p className="text-sm text-muted-foreground leading-relaxed">{item.desc}</p>
                            </div>
                        ))}
                    </div>
                    <Callout variant="tip" title="Keep instruction files short">
                        Instruction files are injected into every session. Keep them concise — a few hundred words at most. Long instruction files consume context window space and tend to get ignored by the model as they get buried. Put detailed design docs in your wiki and reference them from a short AGENTS.md.
                    </Callout>
                </section>

                <PageNav href="/ai/sessions" />
            </div>
        </DocsLayout>
    );
}
