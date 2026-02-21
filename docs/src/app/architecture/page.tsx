import { DocsLayout } from "@/components/layout/DocsLayout";
import { PageNav } from "@/components/docs/PageNav";
import { CodeBlock } from "@/components/docs/CodeBlock";
import { Callout } from "@/components/docs/Callout";
import { ChevronRight } from "lucide-react";
import Link from "next/link";

const toc = [
    { id: "overview", title: "Overview" },
    { id: "core-daemon", title: "Core daemon" },
    { id: "studio-ide", title: "Studio IDE" },
    { id: "worker-system", title: "Worker system" },
    { id: "data-layer", title: "Data layer" },
    { id: "extension-system", title: "Extension system" },
    { id: "data-flow", title: "Request flow" },
    { id: "home-directory", title: "Home directory" },
];

export default function Architecture() {
    return (
        <DocsLayout toc={toc}>
            <div className="flex flex-col gap-10">

                <div className="flex items-center gap-1.5 text-xs text-muted-foreground font-medium">
                    <Link href="/" className="hover:text-foreground transition-colors">RootCX</Link>
                    <ChevronRight className="h-3 w-3" />
                    <span className="text-foreground">Architecture</span>
                </div>

                <header className="flex flex-col gap-4">
                    <h1 className="text-4xl font-semibold tracking-tight lg:text-5xl">Architecture</h1>
                    <p className="text-lg text-muted-foreground max-w-2xl leading-relaxed">
                        A deep look at how RootCX is structured — the daemon, Studio, worker processes, and the data layer.
                    </p>
                </header>

                {/* Overview */}
                <section className="flex flex-col gap-4" id="overview">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Overview</h2>
                    <p className="text-muted-foreground leading-7">
                        RootCX is built around two primary tiers: a <strong className="text-foreground font-medium">Runtime tier</strong> managed by the Core daemon, and a <strong className="text-foreground font-medium">Development tier</strong> used by Studio and application code. Both communicate over a local HTTP API.
                    </p>
                    <CodeBlock language="text" code={`┌─────────────────────────────────────────────────────────────┐
│                   Development Tier                          │
│                                                             │
│   ┌─────────────────────────┐   ┌──────────────────────┐  │
│   │   RootCX Studio (IDE)   │   │  Your App Frontend   │  │
│   │   Tauri v2 + React 19   │   │  React + SDK hooks   │  │
│   └────────────┬────────────┘   └──────────┬───────────┘  │
│                └──────────────┬─────────────┘              │
│                               │ HTTP REST                  │
├───────────────────────────────┼────────────────────────────┤
│                   Runtime Tier                             │
│                                                             │
│               ┌──────────────────────────┐                 │
│               │     RootCX Core          │                 │
│               │  Rust + Axum · port 9100 │                 │
│               │                          │                 │
│               │  ┌──────┐ ┌────────┐    │                 │
│               │  │ Auth │ │  RBAC  │    │                 │
│               │  └──────┘ └────────┘    │                 │
│               │  ┌───────┐ ┌───────┐    │                 │
│               │  │ Audit │ │ Logs  │    │                 │
│               │  └───────┘ └───────┘    │                 │
│               │  ┌─────────────────┐    │                 │
│               │  │  Worker Mgr     │    │                 │
│               │  │  Bun Supervisor │    │                 │
│               │  └─────────────────┘    │                 │
│               │  ┌─────────────────┐    │                 │
│               │  │  Job Scheduler  │    │                 │
│               │  └─────────────────┘    │                 │
│               └──────────┬───────────────┘                 │
│                          │ libpq / TCP                     │
│               ┌──────────┴───────────────┐                 │
│               │   PostgreSQL 18 · 5480   │                 │
│               └──────────────────────────┘                 │
└─────────────────────────────────────────────────────────────┘`} />
                </section>

                {/* Core daemon */}
                <section className="flex flex-col gap-6" id="core-daemon">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Core daemon</h2>
                    <p className="text-muted-foreground leading-7">
                        The Core daemon (<code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">rootcx-core</code>) is a single statically-linked Rust binary. It is the orchestration layer for everything: databases, APIs, workers, and governance.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        On startup, Core performs a sequenced boot:
                    </p>
                    <ol className="flex flex-col gap-3 ml-4">
                        {[
                            "Resolve bundled PostgreSQL and Bun binaries from the resources directory.",
                            "Initialize or validate the data directory at ~/RootCX/.",
                            "Start the embedded PostgreSQL instance on port 5480.",
                            "Connect to the database and bootstrap the system schema (rootcx_system).",
                            "Load and bootstrap all runtime extensions in order: Auth → Logs → Audit → RBAC.",
                            "Initialize the SecretManager (AES-256-GCM) from the master key.",
                            "Start the WorkerManager for application worker supervision.",
                            "Spawn the job scheduler (500 ms polling interval).",
                            "Start the Axum HTTP server on port 9100.",
                        ].map((step, i) => (
                            <li key={i} className="flex items-start gap-3 text-sm text-muted-foreground leading-relaxed">
                                <span className="mt-0.5 flex h-5 w-5 shrink-0 items-center justify-center rounded-full bg-primary/10 text-xs font-bold text-primary">{i + 1}</span>
                                {step}
                            </li>
                        ))}
                    </ol>
                    <Callout variant="info" title="Port configuration">
                        Core listens on <code>localhost:9100</code> by default. PostgreSQL binds to <code>localhost:5480</code>. Both are local-only and not exposed on the network unless explicitly configured.
                    </Callout>
                </section>

                {/* Studio IDE */}
                <section className="flex flex-col gap-4" id="studio-ide">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Studio IDE</h2>
                    <p className="text-muted-foreground leading-7">
                        RootCX Studio is a desktop application built with <strong className="text-foreground font-medium">Tauri v2</strong> (Rust native shell) and a <strong className="text-foreground font-medium">React 19</strong> frontend. It provides a development environment for building and managing RootCX applications.
                    </p>
                    <div className="grid grid-cols-1 sm:grid-cols-2 gap-3 mt-2">
                        {[
                            { title: "Manifest Editor", desc: "Visual JSON editor with validation and schema preview." },
                            { title: "File Explorer", desc: "Full filesystem explorer for your app's source code." },
                            { title: "Code Editor", desc: "CodeMirror 6 with multi-language support (TypeScript, SQL, JSON, TOML)." },
                            { title: "Forge Panel", desc: "One-click deployment of worker code to the running daemon." },
                            { title: "Console", desc: "Real-time log streaming from all running workers via SSE." },
                            { title: "Command Palette", desc: "Keyboard-driven commands for all Studio operations." },
                        ].map((f, i) => (
                            <div key={i} className="rounded-lg border border-border bg-[#111] p-4">
                                <p className="font-medium text-foreground text-sm mb-1">{f.title}</p>
                                <p className="text-xs text-muted-foreground leading-relaxed">{f.desc}</p>
                            </div>
                        ))}
                    </div>
                    <p className="text-muted-foreground leading-7">
                        Studio bundles the Core binary and automatically starts it when the IDE is launched, running a health-check loop until the daemon reports ready before enabling the UI.
                    </p>
                </section>

                {/* Worker system */}
                <section className="flex flex-col gap-4" id="worker-system">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Worker system</h2>
                    <p className="text-muted-foreground leading-7">
                        Workers are Node.js/Bun processes that run your application's custom business logic. Each application can have one worker. The Core daemon manages the entire worker lifecycle: spawning, crash recovery, IPC routing, and log aggregation.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        Communication between Core and a worker happens via a <strong className="text-foreground font-medium">JSON-line IPC protocol</strong> over stdin/stdout. This avoids networking overhead and keeps inter-process communication simple and auditable.
                    </p>
                    <CodeBlock language="text" code={`Worker Lifecycle

[Stopped] ──── start() ──►  [Starting]
                                 │
                           Discover handshake
                                 │
                             [Running] ◄──── RPC calls
                                 │           Job dispatch
                                 │           Log streaming
                           crash or stop()
                                 │
                    ┌────────────┴────────────┐
                    │                         │
               [Stopping]                 [Crashed]
                    │                         │
               [Stopped]             restart w/ backoff
                                     (max 5 / 60s)`} />
                    <p className="text-muted-foreground leading-7">
                        The supervisor implements exponential backoff on restarts, and declares a worker "crashed" (not auto-restarted) after 5 failures within 60 seconds. Secrets are decrypted and injected as environment variables before each spawn.
                    </p>
                </section>

                {/* Data layer */}
                <section className="flex flex-col gap-4" id="data-layer">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Data layer</h2>
                    <p className="text-muted-foreground leading-7">
                        RootCX embeds a full <strong className="text-foreground font-medium">PostgreSQL 18</strong> instance that it manages entirely. Data is organized into PostgreSQL schemas — one per application (<code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">app_{"{appId}"}</code>) plus a reserved system schema (<code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">rootcx_system</code>).
                    </p>
                    <div className="rounded-lg border border-border overflow-hidden">
                        <table className="w-full text-sm">
                            <thead>
                                <tr className="border-b border-border bg-[#0d0d0d]">
                                    <th className="px-4 py-3 text-left font-semibold">Schema</th>
                                    <th className="px-4 py-3 text-left font-semibold">Contents</th>
                                </tr>
                            </thead>
                            <tbody>
                                {[
                                    ["rootcx_system", "users, sessions, apps, jobs, secrets, audit_log, rbac_roles, rbac_assignments, rbac_policies"],
                                    ["app_{appId}", "All entity tables for the installed application"],
                                ].map(([schema, contents], i) => (
                                    <tr key={i} className="border-b border-border/50 last:border-0">
                                        <td className="px-4 py-3 font-mono text-xs text-primary">{schema}</td>
                                        <td className="px-4 py-3 text-xs text-muted-foreground">{contents}</td>
                                    </tr>
                                ))}
                            </tbody>
                        </table>
                    </div>
                    <p className="text-muted-foreground leading-7">
                        Every entity table gets three automatic columns: <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">id</code> (UUID v4 primary key), <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">created_at</code>, and <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">updated_at</code> (both <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">TIMESTAMPTZ</code>). When ownership RBAC is enabled, an <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">owner_id</code> UUID column is added automatically.
                    </p>
                </section>

                {/* Extension system */}
                <section className="flex flex-col gap-4" id="extension-system">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Extension system</h2>
                    <p className="text-muted-foreground leading-7">
                        Core features like authentication, RBAC, audit logging, and log streaming are implemented as <strong className="text-foreground font-medium">Runtime Extensions</strong>. Each extension follows a trait contract that allows it to hook into the app lifecycle.
                    </p>
                    <CodeBlock language="rust" code={`pub trait RuntimeExtension: Send + Sync {
    fn name(&self) -> &str;

    // Called once during daemon startup
    async fn bootstrap(&self, pool: &PgPool) -> Result<()>;

    // Called after each entity table is created
    async fn on_table_created(
        &self, pool: &PgPool, manifest: &AppManifest,
        schema: &str, table: &str
    ) -> Result<()>;

    // Called once after all entity tables are created
    async fn on_app_installed(
        &self, pool: &PgPool, manifest: &AppManifest
    ) -> Result<()>;

    // Optional: contribute routes to the HTTP server
    fn routes(&self) -> Option<Router<SharedRuntime>>;
}`} />
                    <p className="text-muted-foreground leading-7">
                        The four built-in extensions are loaded in dependency order: <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">AuthExtension</code> → <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">LogsExtension</code> → <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">AuditExtension</code> → <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">RbacExtension</code>.
                    </p>
                </section>

                {/* Request flow */}
                <section className="flex flex-col gap-4" id="data-flow">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Request flow</h2>
                    <p className="text-muted-foreground leading-7">
                        A typical authenticated CRUD request flows through the following layers:
                    </p>
                    <ol className="flex flex-col gap-3 ml-4 mt-2">
                        {[
                            { title: "JWT extraction", desc: "The Authorization header is parsed. If present, the JWT is decoded and the user ID + username extracted into request context." },
                            { title: "RBAC check", desc: "The requested action (create/read/update/delete) and entity name are resolved against the in-memory policy cache for the user's roles." },
                            { title: "Ownership filter", desc: "If the matching policy has ownership: true, a WHERE owner_id = $user_id clause is applied to all SQL queries." },
                            { title: "SQL execution", desc: "A parameterized query is constructed and executed via the SQLx connection pool." },
                            { title: "Audit trigger", desc: "A PostgreSQL trigger fires on INSERT/UPDATE/DELETE, appending a row to the audit_log table with old and new record snapshots." },
                            { title: "Response", desc: "The record is serialized to JSON and returned with the appropriate HTTP status." },
                        ].map((step, i) => (
                            <li key={i} className="flex items-start gap-3 text-sm leading-relaxed">
                                <span className="mt-0.5 flex h-5 w-5 shrink-0 items-center justify-center rounded-full bg-primary/10 text-xs font-bold text-primary">{i + 1}</span>
                                <div>
                                    <span className="font-medium text-foreground">{step.title} — </span>
                                    <span className="text-muted-foreground">{step.desc}</span>
                                </div>
                            </li>
                        ))}
                    </ol>
                </section>

                {/* Home directory */}
                <section className="flex flex-col gap-4" id="home-directory">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Home directory</h2>
                    <p className="text-muted-foreground leading-7">
                        RootCX stores all runtime data under <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">~/RootCX/</code> (platform home directory). The structure is:
                    </p>
                    <CodeBlock language="text" code={`~/RootCX/
├── bin/
│   └── rootcx-core          # Symlink to the daemon binary
├── config/
│   ├── jwt.key              # 32-byte JWT signing key (auto-generated)
│   └── master.key           # 32-byte AES master key (auto-generated)
├── data/
│   └── pg/                  # PostgreSQL data directory (PGDATA)
│       ├── PG_VERSION
│       ├── pg_hba.conf
│       └── ...
├── apps/
│   ├── myapp/               # Deployed worker code per app
│   │   ├── package.json
│   │   └── index.ts
│   └── crm/
│       └── ...
└── logs/
    └── runtime.log          # Daemon stdout log`} />
                    <Callout variant="warning" title="Key security">
                        The <code>jwt.key</code> and <code>master.key</code> files are auto-generated on first boot with 32 bytes of cryptographic randomness. Back them up securely. Losing the master key means encrypted secrets cannot be decrypted.
                    </Callout>
                </section>

                <PageNav href="/architecture" />
            </div>
        </DocsLayout>
    );
}
