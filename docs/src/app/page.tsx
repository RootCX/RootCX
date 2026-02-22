import { DocsLayout } from "@/components/layout/DocsLayout";
import { PageNav } from "@/components/docs/PageNav";
import {
    ChevronRight,
    Code,
    Database,
    Terminal,
    ArrowRight,
} from "lucide-react";
import Link from "next/link";

const toc = [
    { id: "what-is-rootcx", title: "What is RootCX?" },
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
                        An open-source, local-first platform for building enterprise internal software and AI agents. Describe your intent to the AI Forge or write code directly.
                    </p>
                </header>

                {/* Intro */}
                <section className="flex flex-col gap-4 -mt-2">
                    <p className="text-muted-foreground leading-7">
                        RootCX is designed to power fleets of interconnected internal software and AI agents. Create one app or several hundred: every app runs on the same Core, a single self-contained binary (<code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">rootcx-core</code>) embedding a fully managed PostgreSQL 18 instance. Apps share the same database through isolated namespaces for native interoperability, and inherit the same enterprise governance layer for consistent security across your entire organization.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        That governance layer includes authentication (JWT, SSO/OIDC), RBAC, immutable audit logs, and an AES-256 encrypted secret vault, alongside automatic CRUD APIs, isolated backend processes, and background job scheduling.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        <strong className="text-foreground font-medium">RootCX Studio</strong> is your build station: a native desktop app where you write code, deploy backends, monitor live logs, and manage your entire fleet from a single window.
                    </p>

                </section>

                {/* Next steps */}
                <section className="flex flex-col gap-4" id="next-steps">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">
                        Next steps
                    </h2>
                    <div className="mt-2">
                        <Link
                            href="/quickstart"
                            className="inline-flex items-center gap-2 rounded-lg bg-primary px-4 py-2 text-sm font-medium text-white transition-opacity hover:opacity-90"
                        >
                            Quick Start <ArrowRight className="h-4 w-4" />
                        </Link>
                    </div>
                    <div className="grid grid-cols-1 sm:grid-cols-3 gap-3 mt-2">
                        {[
                            {
                                title: "Studio",
                                href: "/studio",
                                desc: "Native desktop IDE. Write code, deploy, monitor logs, and manage your fleet from a single window.",
                                icon: <Terminal className="h-4 w-4" />,
                            },
                            {
                                title: "Core",
                                href: "/architecture",
                                desc: "Single Rust binary embedding PostgreSQL 18. Auth, CRUD, RBAC, audit, secrets, jobs, and backend isolation.",
                                icon: <Database className="h-4 w-4" />,
                            },
                            {
                                title: "Runtime SDK",
                                href: "/sdk",
                                desc: "React hooks for auth sessions, data fetching, permissions, and real-time status.",
                                icon: <Code className="h-4 w-4" />,
                            },
                        ].map((item, i) => (
                            <Link
                                key={i}
                                href={item.href}
                                className="group flex flex-col gap-2.5 p-5 rounded-xl border border-border bg-[#111] hover:bg-[#141414] transition-all hover:border-primary/40"
                            >
                                <div className="flex items-center justify-between">
                                    <div className="h-9 w-9 shrink-0 bg-primary/10 flex items-center justify-center rounded-lg text-primary">
                                        {item.icon}
                                    </div>
                                    <ArrowRight className="h-4 w-4 text-muted-foreground/30 group-hover:text-primary/60 transition-colors" />
                                </div>
                                <h3 className="font-semibold text-foreground">{item.title}</h3>
                                <p className="text-sm text-muted-foreground leading-relaxed">{item.desc}</p>
                            </Link>
                        ))}
                    </div>
                </section>

                <PageNav href="/" />
            </div>
        </DocsLayout>
    );
}
