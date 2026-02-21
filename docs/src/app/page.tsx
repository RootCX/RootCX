import { DocsLayout } from "@/components/layout/DocsLayout";
import { PageNav } from "@/components/docs/PageNav";
import {
    ChevronRight,
    Blocks,
    Shield,
    Activity,
    Settings,
    Code,
    Users,
    Zap,
    Database,
    Terminal,
    ArrowRight,
    Bot,
} from "lucide-react";
import Link from "next/link";

const toc = [
    { id: "what-is-rootcx", title: "What is RootCX?" },
    { id: "how-it-works", title: "How it works" },
    { id: "core-concepts", title: "Core concepts" },
    { id: "building-with-ai", title: "Building with AI" },
    { id: "next-steps", title: "Next steps" },
];

export default function Home() {
    return (
        <DocsLayout toc={toc}>
            <div className="flex flex-col gap-10">

                {/* Breadcrumb */}
                <div className="flex items-center gap-1.5 text-xs text-muted-foreground font-medium">
                    <span>RootCX</span>
                    <ChevronRight className="h-3 w-3" />
                    <span className="text-foreground">Introduction</span>
                </div>

                {/* Header */}
                <header className="flex flex-col gap-4" id="what-is-rootcx">
                    <h1 className="text-4xl font-semibold tracking-tight text-foreground lg:text-5xl">
                        What is RootCX?
                    </h1>
                    <p className="text-lg text-muted-foreground max-w-2xl leading-relaxed">
                        An open-source, local-first platform for building custom internal software and AI agents — with a built-in database, automatic APIs, native tools for AI/MCP, an enterprise governance layer, and a Studio IDE.
                    </p>
                </header>

                {/* Intro */}
                <section className="flex flex-col gap-4 -mt-2">
                    <p className="text-muted-foreground leading-7">
                        RootCX bundles everything your team needs to build, ship, and govern business applications and AI agents — without relying on third-party hosted infrastructure. You define your data model in a simple JSON manifest, and RootCX takes care of the rest: schema creation, CRUD APIs, role-based access control, audit logging, secret management, and a background backend system for custom logic.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        The platform ships as a single self-contained binary (<code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">rootcx-core</code>) that embeds a fully managed PostgreSQL 18 instance. Alongside it, the <strong className="text-foreground font-medium">RootCX Studio</strong> desktop IDE lets you describe your intent, automatically scaffolding schemas, backend logic, and providing a cohesive, rich UI out of the box.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        <strong>Scale your business with an interconnected fleet.</strong> RootCX is built to power not just one, but tens, hundreds, or thousands of interconnected applications within your enterprise. All applications connect to the same core database, sharing data securely through different namespaces. This allows you to perform highly optimized cross-joins using performant SQL, and enables seamless interoperability between your custom software and AI Agents via MCP and APIs.
                    </p>

                    <div className="mt-4">
                        <Link
                            href="/quickstart"
                            className="inline-flex items-center gap-2 rounded-lg bg-primary px-4 py-2 text-sm font-medium text-white transition-opacity hover:opacity-90"
                        >
                            Quick Start <ArrowRight className="h-4 w-4" />
                        </Link>
                    </div>
                </section>

                {/* How it works */}
                <section className="flex flex-col gap-6" id="how-it-works">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">
                        How it works
                    </h2>
                    <p className="text-muted-foreground leading-7">
                        RootCX transforms your intent into enterprise-ready software. Instead of doing the heavy lifting manually, the Studio scaffolds the entire stack for you — delivering a rich, cohesive UI and a robust foundation instantly.
                    </p>
                    <div className="grid grid-cols-1 gap-0 rounded-xl border border-border overflow-hidden">
                        {[
                            {
                                step: "01",
                                title: "Describe your intent",
                                desc: "Use the Studio AI Forge or write a JSON manifest describing your application's data model, roles, and business needs. RootCX understands your intent.",
                                icon: <Bot className="h-5 w-5" />,
                            },
                            {
                                step: "02",
                                title: "Automatic App Generation",
                                desc: "The Studio does the heavy lifting, instantly scaffolding an up-and-running application with a rich, consistent UI and enterprise-grade architecture.",
                                icon: <Zap className="h-5 w-5" />,
                            },
                            {
                                step: "03",
                                title: "Core & Foundation Provisioning",
                                desc: "RootCX Core automatically provisions the PostgreSQL schema, applies RBAC policies, registers audit triggers, and exposes native tools to AI/MCP workflows.",
                                icon: <Database className="h-5 w-5" />,
                            },
                            {
                                step: "04",
                                title: "Extend & Connect",
                                desc: "Deploy custom isolated backend processes, connect other apps within your fleet via native APIs, or use MCP to let any AI agent securely interact with your internal software built.",
                                icon: <Code className="h-5 w-5" />,
                            },
                        ].map((s, i) => (
                            <div key={i} className="flex items-start gap-5 p-6 border-b border-border last:border-0 hover:bg-white/[0.02] transition-colors">
                                <div className="flex flex-col items-center gap-2">
                                    <span className="text-xs font-mono font-bold text-primary/60">{s.step}</span>
                                    <div className="h-9 w-9 shrink-0 bg-primary/10 flex items-center justify-center rounded-lg text-primary">
                                        {s.icon}
                                    </div>
                                </div>
                                <div className="flex flex-col gap-1 pt-1">
                                    <h3 className="font-semibold text-foreground">{s.title}</h3>
                                    <p className="text-sm text-muted-foreground leading-relaxed">{s.desc}</p>
                                </div>
                            </div>
                        ))}
                    </div>
                </section>

                {/* Core concepts */}
                <section className="flex flex-col gap-6" id="core-concepts">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">
                        Core concepts
                    </h2>
                    <div className="grid grid-cols-1 md:grid-cols-2 gap-3">
                        {[
                            {
                                href: "/architecture",
                                title: "Self-hosted Core",
                                desc: "A Rust daemon that manages PostgreSQL, APIs, and the isolated backend process.",
                                icon: <Database className="h-4 w-4" />,
                            },
                            {
                                href: "/modules/authentication",
                                title: "Authentication",
                                desc: "Native JWT-based auth with user registration, sessions, and token refresh.",
                                icon: <Users className="h-4 w-4" />,
                            },
                            {
                                href: "/concepts/permissions",
                                title: "Roles & Permissions",
                                desc: "Declarative RBAC policies with role inheritance and row-level ownership.",
                                icon: <Shield className="h-4 w-4" />,
                            },
                            {
                                href: "/modules/audit",
                                title: "Audit Logs",
                                desc: "Immutable change history powered by native PostgreSQL triggers.",
                                icon: <Activity className="h-4 w-4" />,
                            },
                            {
                                href: "/modules/secrets",
                                title: "Secret Vault",
                                desc: "AES-256-GCM encrypted secrets injected into the backend process.",
                                icon: <Settings className="h-4 w-4" />,
                            },
                            {
                                href: "/modules/backend",
                                title: "Backend & RPC",
                                desc: "Deploy custom TypeScript/JavaScript logic as a mutualized isolated process.",
                                icon: <Code className="h-4 w-4" />,
                            },
                            {
                                href: "/ai",
                                title: "AI Forge",
                                desc: "Build internal software in conversation. Connect any LLM — Anthropic, OpenAI, Copilot, or local.",
                                icon: <Bot className="h-4 w-4" />,
                            },
                        ].map((c, i) => (
                            <Link
                                key={i}
                                href={c.href}
                                className="group flex flex-col gap-2.5 p-5 rounded-xl border border-border bg-[#111] hover:bg-[#141414] transition-all hover:border-primary/40"
                            >
                                <div className="flex items-center justify-between">
                                    <div className="h-9 w-9 shrink-0 bg-primary/10 flex items-center justify-center rounded-lg text-primary">
                                        {c.icon}
                                    </div>
                                    <ArrowRight className="h-4 w-4 text-muted-foreground/30 group-hover:text-primary/60 transition-colors" />
                                </div>
                                <h3 className="font-semibold text-foreground">{c.title}</h3>
                                <p className="text-sm text-muted-foreground leading-relaxed">{c.desc}</p>
                            </Link>
                        ))}
                    </div>
                </section>

                {/* Building with AI spotlight */}
                <section className="flex flex-col gap-4" id="building-with-ai">
                    <div className="rounded-2xl border border-primary/30 bg-primary/5 p-6 flex flex-col gap-4">
                        <div className="flex items-center gap-3">
                            <div className="h-10 w-10 shrink-0 bg-primary/15 flex items-center justify-center rounded-xl text-primary">
                                <Bot className="h-5 w-5" />
                            </div>
                            <div>
                                <h2 className="text-lg font-semibold text-foreground">Building with AI Forge</h2>
                                <p className="text-sm text-muted-foreground">The fastest way to build internal software</p>
                            </div>
                        </div>
                        <p className="text-muted-foreground leading-relaxed">
                            RootCX was designed for a world where AI writes most of the code. The <strong className="text-foreground font-medium">AI Forge</strong> panel in Studio connects to <strong className="text-foreground font-medium">OpenCode</strong> — an open-source AI coding engine — and lets you build backend logic, schemas, and endpoints through conversation. Connect Anthropic Claude, OpenAI, GitHub Copilot, or any local model. No vendor lock-in.
                        </p>
                        <div className="flex flex-wrap gap-2">
                            <Link
                                href="/ai"
                                className="inline-flex items-center gap-2 rounded-lg bg-primary px-3.5 py-1.5 text-sm font-medium text-white transition-opacity hover:opacity-90"
                            >
                                Building with AI <ArrowRight className="h-3.5 w-3.5" />
                            </Link>
                            <Link
                                href="/ai/providers"
                                className="inline-flex items-center gap-2 rounded-lg border border-border bg-[#141414] px-3.5 py-1.5 text-sm font-medium text-foreground transition-colors hover:bg-[#1a1a1a]"
                            >
                                Configure providers
                            </Link>
                        </div>
                    </div>
                </section>

                {/* Next steps */}
                <section className="flex flex-col gap-4" id="next-steps">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">
                        Next steps
                    </h2>
                    <p className="text-muted-foreground leading-7">
                        Ready to get started? Follow the quick start guide to spin up your first application in minutes, or dive into the architecture overview to understand how all the pieces fit together.
                    </p>
                    <div className="flex flex-wrap gap-3 mt-2">
                        <Link
                            href="/quickstart"
                            className="inline-flex items-center gap-2 rounded-lg bg-primary px-4 py-2 text-sm font-medium text-white transition-opacity hover:opacity-90"
                        >
                            Quick Start <ArrowRight className="h-4 w-4" />
                        </Link>
                        <Link
                            href="/architecture"
                            className="inline-flex items-center gap-2 rounded-lg border border-border bg-[#141414] px-4 py-2 text-sm font-medium text-foreground transition-colors hover:bg-[#1a1a1a]"
                        >
                            Architecture overview
                        </Link>
                    </div>
                </section>

                <PageNav href="/" />
            </div>
        </DocsLayout>
    );
}
