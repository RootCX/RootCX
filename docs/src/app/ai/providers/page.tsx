import { DocsLayout } from "@/components/layout/DocsLayout";
import { PageNav } from "@/components/docs/PageNav";
import { CodeBlock } from "@/components/docs/CodeBlock";
import { Callout } from "@/components/docs/Callout";
import { ChevronRight } from "lucide-react";
import Link from "next/link";

const toc = [
    { id: "overview", title: "Overview" },
    { id: "anthropic", title: "Anthropic Claude" },
    { id: "openai", title: "OpenAI" },
    { id: "github-copilot", title: "GitHub Copilot" },
    { id: "local-models", title: "Local & custom models" },
    { id: "model-selection", title: "Model selection" },
    { id: "switching-providers", title: "Switching providers" },
];

export default function ProvidersPage() {
    return (
        <DocsLayout toc={toc}>
            <div className="flex flex-col gap-10">

                <div className="flex items-center gap-1.5 text-xs text-muted-foreground font-medium">
                    <Link href="/" className="hover:text-foreground transition-colors">RootCX</Link>
                    <ChevronRight className="h-3 w-3" />
                    <Link href="/ai" className="hover:text-foreground transition-colors">Building with AI</Link>
                    <ChevronRight className="h-3 w-3" />
                    <span className="text-foreground">LLM Providers</span>
                </div>

                <header className="flex flex-col gap-4" id="overview">
                    <h1 className="text-4xl font-semibold tracking-tight lg:text-5xl">LLM Providers</h1>
                    <p className="text-lg text-muted-foreground max-w-2xl leading-relaxed">
                        AI Forge supports any LLM you already have access to. Bring your own API key or connect to GitHub Copilot — no vendor lock-in, no data sent to RootCX servers.
                    </p>
                </header>

                <section className="flex flex-col gap-4">
                    <p className="text-muted-foreground leading-7">
                        Provider configuration lives in OpenCode — the local AI engine embedded in Studio. Each provider requires credentials (an API key or auth token) that you configure once. Your credentials are stored locally by OpenCode and are never transmitted to RootCX servers.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        You can configure providers directly from the AI Forge panel in Studio, or by editing the OpenCode configuration file. The active provider and model is shown at the top of the Forge panel and can be changed at any time without restarting Studio.
                    </p>
                </section>

                <section className="flex flex-col gap-4" id="anthropic">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Anthropic Claude</h2>
                    <p className="text-muted-foreground leading-7">
                        Anthropic's Claude models are the recommended default for Forge. Claude Sonnet 4 and above offer the best balance of coding speed, context handling, and reasoning quality for the kinds of tasks Forge performs — reading large codebases, generating structured JSON manifests, and writing TypeScript workers.
                    </p>
                    <div className="rounded-xl border border-border overflow-hidden">
                        <div className="border-b border-border bg-[#0d0d0d] px-4 py-3">
                            <span className="font-semibold text-sm text-foreground">Available models</span>
                        </div>
                        <div className="flex flex-col">
                            {[
                                { model: "claude-opus-4-6", label: "Claude Opus 4.6", note: "Most capable — complex multi-step tasks, large codebases, extended reasoning" },
                                { model: "claude-sonnet-4-6", label: "Claude Sonnet 4.6", note: "Best balance of speed and capability — recommended for most workflows" },
                                { model: "claude-haiku-4-5", label: "Claude Haiku 4.5", note: "Fastest and most cost-efficient — quick edits, code review, Q&A" },
                            ].map((m, i) => (
                                <div key={i} className="flex items-start justify-between gap-4 px-4 py-3 border-b border-border/50 last:border-0">
                                    <div>
                                        <code className="font-mono text-xs text-primary">{m.model}</code>
                                        <p className="text-xs text-muted-foreground mt-0.5">{m.label}</p>
                                    </div>
                                    <p className="text-xs text-muted-foreground text-right max-w-xs leading-relaxed">{m.note}</p>
                                </div>
                            ))}
                        </div>
                    </div>
                    <CodeBlock
                        language="bash"
                        filename="Configure via environment variable"
                        code={`export ANTHROPIC_API_KEY="sk-ant-..."`}
                    />
                    <Callout variant="tip" title="Extended thinking">
                        Claude Opus 4.6 supports extended thinking — visible reasoning traces that show how the AI plans its approach before writing code. Enable it from the model settings in the Forge panel for complex architectural tasks.
                    </Callout>
                </section>

                <section className="flex flex-col gap-4" id="openai">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">OpenAI</h2>
                    <p className="text-muted-foreground leading-7">
                        OpenAI's GPT-4o and o-series models are fully supported. GPT-4o is a strong all-around coding model; the o-series models (o1, o3) excel at reasoning-heavy tasks like designing normalized schemas or debugging complex logic.
                    </p>
                    <div className="rounded-xl border border-border overflow-hidden">
                        <div className="border-b border-border bg-[#0d0d0d] px-4 py-3">
                            <span className="font-semibold text-sm text-foreground">Available models</span>
                        </div>
                        <div className="flex flex-col">
                            {[
                                { model: "gpt-4o", note: "Fast, multimodal, strong coding capability" },
                                { model: "o1", note: "Deliberate reasoning — best for architecture and complex debugging" },
                                { model: "o3-mini", note: "Fast reasoning at lower cost" },
                            ].map((m, i) => (
                                <div key={i} className="flex items-start justify-between gap-4 px-4 py-3 border-b border-border/50 last:border-0">
                                    <code className="font-mono text-xs text-primary">{m.model}</code>
                                    <p className="text-xs text-muted-foreground text-right max-w-xs leading-relaxed">{m.note}</p>
                                </div>
                            ))}
                        </div>
                    </div>
                    <CodeBlock
                        language="bash"
                        filename="Configure via environment variable"
                        code={`export OPENAI_API_KEY="sk-..."`}
                    />
                </section>

                <section className="flex flex-col gap-4" id="github-copilot">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">GitHub Copilot</h2>
                    <p className="text-muted-foreground leading-7">
                        If your team already has a GitHub Copilot subscription, you can use it with Forge at no additional LLM cost. Authentication uses your existing GitHub credentials — no separate API key required.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        OpenCode handles the Copilot OAuth flow automatically when you select GitHub Copilot as your provider in the Forge panel. You will be prompted to authorize in your browser on first use.
                    </p>
                    <Callout variant="info" title="Model availability">
                        The models available through GitHub Copilot depend on your subscription tier. Enterprise subscribers typically have access to Claude Sonnet and GPT-4o through Copilot. Check your Copilot settings for the current model list.
                    </Callout>
                </section>

                <section className="flex flex-col gap-4" id="local-models">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Local & custom models</h2>
                    <p className="text-muted-foreground leading-7">
                        Any OpenAI-compatible HTTP endpoint works as a provider. This includes locally running inference servers like <strong className="text-foreground font-medium">Ollama</strong> and <strong className="text-foreground font-medium">LM Studio</strong>, as well as cloud-hosted compatible APIs.
                    </p>
                    <CodeBlock
                        language="json"
                        filename="opencode.json — custom provider"
                        code={`{
  "provider": "custom",
  "baseUrl": "http://localhost:11434/v1",
  "model": "llama3.2:3b",
  "apiKey": "ollama"
}`}
                    />
                    <Callout variant="warning" title="Local model limitations">
                        Local models with small context windows (under 32k tokens) may struggle with large codebases. Forge works best with models that support at least 64k context. For production internal tools, cloud-hosted models with large context windows deliver significantly better results.
                    </Callout>
                </section>

                <section className="flex flex-col gap-4" id="model-selection">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Model selection</h2>
                    <p className="text-muted-foreground leading-7">
                        The active model is displayed at the top of the Forge panel. Click it to open the model picker — a searchable list of all models available from your connected providers.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        Different agents can use different models. For example, you might use a fast, cheap model for the explore agent (which does a lot of file reading) and a more capable model for the build agent (which writes the actual code). Configure per-agent models from the Forge settings panel.
                    </p>
                    <div className="flex flex-col gap-2">
                        <p className="text-sm font-medium text-foreground">Recommended model pairings by task:</p>
                        {[
                            { task: "Designing a data model or RBAC policy", model: "Claude Opus 4.6 or o1", reason: "Benefits from deep reasoning about constraints and edge cases" },
                            { task: "Implementing a worker or writing TypeScript", model: "Claude Sonnet 4.6 or GPT-4o", reason: "Fast, accurate code generation with good instruction following" },
                            { task: "Explaining code or answering questions", model: "Claude Haiku 4.5 or o3-mini", reason: "Low latency for conversational back-and-forth" },
                            { task: "Exploring and reading large codebases", model: "Claude Sonnet 4.6", reason: "Long context handling at reasonable speed" },
                        ].map((r, i) => (
                            <div key={i} className="rounded-lg border border-border bg-[#111] p-4 flex flex-col gap-1.5">
                                <div className="flex items-start justify-between gap-3">
                                    <p className="text-sm font-medium text-foreground">{r.task}</p>
                                    <code className="shrink-0 text-xs font-mono text-primary bg-primary/10 rounded px-1.5 py-0.5">{r.model}</code>
                                </div>
                                <p className="text-xs text-muted-foreground">{r.reason}</p>
                            </div>
                        ))}
                    </div>
                </section>

                <section className="flex flex-col gap-4" id="switching-providers">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Switching providers</h2>
                    <p className="text-muted-foreground leading-7">
                        You can switch providers or models at any time — even mid-session. The conversation history is preserved; only future messages will use the new model. This lets you start a task with a fast model and switch to a more capable one when you hit a complex problem.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        Provider status is shown in the Forge panel header. A green dot indicates the provider is connected and the API key is valid. A red dot means the provider is unreachable or the key is invalid — hover over it for the error details.
                    </p>
                </section>

                <PageNav href="/ai/providers" />
            </div>
        </DocsLayout>
    );
}
