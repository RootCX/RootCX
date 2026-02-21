import { DocsLayout } from "@/components/layout/DocsLayout";
import { PageNav } from "@/components/docs/PageNav";
import { CodeBlock } from "@/components/docs/CodeBlock";
import { Callout } from "@/components/docs/Callout";
import { PropertiesTable } from "@/components/docs/PropertiesTable";
import { ChevronRight } from "lucide-react";
import Link from "next/link";

const toc = [
    { id: "overview", title: "Overview" },
    { id: "runtime-struct", title: "The Runtime Struct" },
    { id: "boot-sequence", title: "Boot Sequence" },
    { id: "system-schema", title: "System Schema" },
    { id: "http-server", title: "HTTP API Server" },
    { id: "connection-pool", title: "Connection Pool" },
    { id: "status-endpoint", title: "Status Endpoint" },
    { id: "health-endpoint", title: "Health Check" },
    { id: "platform-support", title: "Platform Support" },
    { id: "related", title: "Related Pages" },
];

export default function RuntimePage() {
    return (
        <DocsLayout toc={toc}>
            <div className="flex flex-col gap-10">
                <div className="flex items-center gap-1.5 text-xs text-muted-foreground font-medium">
                    <Link href="/" className="hover:text-foreground transition-colors">RootCX</Link>
                    <ChevronRight className="h-3 w-3" />
                    <Link href="/concepts/manifest" className="hover:text-foreground transition-colors">Core Concepts</Link>
                    <ChevronRight className="h-3 w-3" />
                    <span className="text-foreground">Engine & Runtime</span>
                </div>

                <header className="flex flex-col gap-4">
                    <h1 className="text-4xl font-semibold tracking-tight lg:text-5xl">Engine & Runtime</h1>
                    <p className="text-lg text-muted-foreground max-w-2xl leading-relaxed">
                        The RootCX Runtime is the central process that boots PostgreSQL, provisions the system schema, applies your application manifest, and serves the HTTP API. Understanding the runtime's subsystems and boot sequence helps you operate, debug, and extend your application with confidence.
                    </p>
                </header>

                {/* Overview */}
                <section className="flex flex-col gap-4" id="overview">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Overview</h2>
                    <p className="text-muted-foreground leading-7">
                        RootCX is a <strong className="text-foreground font-medium">self-contained application runtime</strong>. Unlike platforms that require you to provision a separate database, message broker, and web server, RootCX bundles and manages all of these subsystems within a single process. The runtime:
                    </p>
                    <ul className="flex flex-col gap-2 text-muted-foreground leading-7 list-disc list-inside">
                        <li>Embeds and manages a PostgreSQL instance (or connects to an external one in production mode)</li>
                        <li>Bootstraps all system tables and extensions on first start</li>
                        <li>Reads the App Manifest and runs the Schema Sync Engine to provision or update your data model</li>
                        <li>Starts the HTTP API server powered by the Axum web framework</li>
                        <li>Runs the Worker Manager and Job Scheduler for background processing</li>
                        <li>Initializes the Secret Manager for encrypted credential storage</li>
                    </ul>
                    <p className="text-muted-foreground leading-7">
                        All subsystems are coordinated within a single <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">Runtime</code> struct. The runtime is written in Rust for memory safety and performance, and is distributed as a single statically-linked binary for each supported platform.
                    </p>
                </section>

                {/* Runtime Struct */}
                <section className="flex flex-col gap-4" id="runtime-struct">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">The Runtime Struct</h2>
                    <p className="text-muted-foreground leading-7">
                        The <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">Runtime</code> struct is the top-level owner of all subsystem state. It is constructed during the boot sequence and lives for the entire process lifetime. Each subsystem is a named field that can be accessed by the HTTP handlers, worker threads, and scheduler.
                    </p>

                    <div className="overflow-x-auto rounded-lg border border-border">
                        <table className="w-full text-sm">
                            <thead>
                                <tr className="border-b border-border bg-[#0d0d0d]">
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Subsystem</th>
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Type</th>
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Responsibility</th>
                                </tr>
                            </thead>
                            <tbody className="divide-y divide-border/50">
                                <tr className="hover:bg-white/[0.02] transition-colors">
                                    <td className="px-4 py-3 font-mono text-xs text-primary">pool</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">PgPool</td>
                                    <td className="px-4 py-3 text-muted-foreground">SQLx async connection pool to PostgreSQL. Shared across all request handlers and background workers.</td>
                                </tr>
                                <tr className="hover:bg-white/[0.02] transition-colors">
                                    <td className="px-4 py-3 font-mono text-xs text-primary">config</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">RuntimeConfig</td>
                                    <td className="px-4 py-3 text-muted-foreground">Resolved configuration from environment variables and the config file. Includes DB connection string, HTTP port, data directory, and feature flags.</td>
                                </tr>
                                <tr className="hover:bg-white/[0.02] transition-colors">
                                    <td className="px-4 py-3 font-mono text-xs text-primary">worker_manager</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">WorkerManager</td>
                                    <td className="px-4 py-3 text-muted-foreground">Manages the lifecycle of JavaScript/TypeScript worker scripts. Handles registration, RPC dispatch, and graceful shutdown.</td>
                                </tr>
                                <tr className="hover:bg-white/[0.02] transition-colors">
                                    <td className="px-4 py-3 font-mono text-xs text-primary">scheduler</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">Scheduler</td>
                                    <td className="px-4 py-3 text-muted-foreground">Cron-based job scheduler. Reads scheduled job definitions from the system schema and dispatches them to workers at the configured intervals.</td>
                                </tr>
                                <tr className="hover:bg-white/[0.02] transition-colors">
                                    <td className="px-4 py-3 font-mono text-xs text-primary">secret_manager</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">SecretManager</td>
                                    <td className="px-4 py-3 text-muted-foreground">Encrypted key-value store for application secrets. Secrets are stored in the system schema and decrypted at access time using the master key.</td>
                                </tr>
                                <tr className="hover:bg-white/[0.02] transition-colors">
                                    <td className="px-4 py-3 font-mono text-xs text-primary">pg_process</td>
                                    <td className="px-4 py-3 font-mono text-xs text-muted-foreground">Option&lt;PgProcess&gt;</td>
                                    <td className="px-4 py-3 text-muted-foreground">Handle to the embedded PostgreSQL process. Present only when running in embedded mode. None when connected to an external PostgreSQL instance.</td>
                                </tr>
                            </tbody>
                        </table>
                    </div>

                    <Callout variant="info" title="Shared state via Arc">
                        The <code>Runtime</code> struct is wrapped in an <code>Arc</code> and cloned into every Axum request handler. This gives handlers zero-copy access to the connection pool, config, and all subsystems without locks on the hot path.
                    </Callout>
                </section>

                {/* Boot Sequence */}
                <section className="flex flex-col gap-4" id="boot-sequence">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Boot Sequence</h2>
                    <p className="text-muted-foreground leading-7">
                        The runtime boots in nine ordered steps. Each step must succeed before the next begins. If any step fails, the process exits with a non-zero status code and writes a structured error to stderr. The full boot sequence completes in under two seconds on modern hardware for typical application sizes.
                    </p>

                    <div className="flex flex-col gap-0">
                        {[
                            {
                                step: 1,
                                title: "Initialize PostgreSQL",
                                detail: "If running in embedded mode, the runtime locates or extracts the PostgreSQL binaries bundled in the executable. It then checks whether a data directory exists at the configured path. If not, it runs initdb to initialize a fresh PostgreSQL cluster with the correct locale, encoding (UTF-8), and authentication settings.",
                                code: null,
                            },
                            {
                                step: 2,
                                title: "Start PostgreSQL",
                                detail: "Spawns the PostgreSQL server process as a child process, writing its PID to a lock file. The runtime then polls the PostgreSQL socket/port with exponential backoff until the server accepts connections, with a configurable timeout (default: 30 seconds).",
                                code: null,
                            },
                            {
                                step: 3,
                                title: "Connect — Create PgPool",
                                detail: "Establishes the SQLx connection pool. The pool is configured with a minimum of 2 connections and a maximum of 20 (configurable). All connections use prepared statement caching for performance. The pool is the sole interface to the database for all subsequent steps and request handlers.",
                                code: null,
                            },
                            {
                                step: 4,
                                title: "Bootstrap System Schema",
                                detail: "Creates the rootcx_system PostgreSQL schema if it does not exist, then applies the system schema migration — creating all internal tables (apps, users, sessions, rbac_roles, rbac_assignments, rbac_policies, secrets, jobs, audit_log, etc.). Migrations are idempotent and run with IF NOT EXISTS guards.",
                                code: null,
                            },
                            {
                                step: 5,
                                title: "Install Extensions",
                                detail: "Ensures required PostgreSQL extensions are installed: uuid-ossp (for gen_random_uuid()), pgcrypto (for secret encryption), and pg_trgm (for trigram-based full-text search). Each extension is created with CREATE EXTENSION IF NOT EXISTS.",
                                code: null,
                            },
                            {
                                step: 6,
                                title: "Initialize Secret Manager",
                                detail: "Loads or generates the master encryption key. The key is derived from the ROOTCX_MASTER_KEY environment variable or a key file in the data directory. The Secret Manager uses AES-256-GCM to encrypt values at rest in the rootcx_system.secrets table.",
                                code: null,
                            },
                            {
                                step: 7,
                                title: "Start Worker Manager",
                                detail: "Loads all registered worker scripts from the rootcx_system.workers table. For each worker, it initializes a Deno/V8 isolate, evaluates the worker script, and registers the worker's exported RPC handlers. Workers that fail to initialize are logged and skipped — the runtime does not fail to boot if individual workers error.",
                                code: null,
                            },
                            {
                                step: 8,
                                title: "Start Scheduler",
                                detail: "Reads scheduled job definitions from the rootcx_system.scheduled_jobs table and registers them with the in-process cron scheduler. Each job is defined by a cron expression, a worker name, and an optional payload. The scheduler dispatches jobs by invoking the target worker's RPC handler at the configured interval.",
                                code: null,
                            },
                            {
                                step: 9,
                                title: "Start HTTP Server",
                                detail: "Binds the Axum HTTP server to 0.0.0.0:9100 (configurable). Registers all API route groups: auth, data, rbac, workers, jobs, secrets, audit, logs, and the manifest endpoint. Enables CORS with configurable allowed origins, sets the request body size limit to 50 MB, and begins accepting connections.",
                                code: null,
                            },
                        ].map(({ step, title, detail }) => (
                            <div key={step} className="flex gap-4 relative">
                                <div className="flex flex-col items-center">
                                    <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-full border border-border bg-[#0d0d0d] text-xs font-semibold text-foreground z-10">
                                        {step}
                                    </div>
                                    {step < 9 && <div className="w-px flex-1 bg-border/50 my-1" />}
                                </div>
                                <div className={`flex flex-col gap-1 pb-6 ${step === 9 ? "pb-0" : ""}`}>
                                    <p className="text-sm font-semibold text-foreground pt-1">{title}</p>
                                    <p className="text-sm text-muted-foreground leading-6">{detail}</p>
                                </div>
                            </div>
                        ))}
                    </div>

                    <Callout variant="tip" title="Boot logs">
                        Each boot step emits a structured log line at the <code>INFO</code> level. If a step fails, a detailed error is logged at <code>ERROR</code> level with the underlying cause. Set the <code>ROOTCX_LOG</code> environment variable to <code>debug</code> for verbose output during troubleshooting.
                    </Callout>
                </section>

                {/* System Schema */}
                <section className="flex flex-col gap-4" id="system-schema">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">System Schema</h2>
                    <p className="text-muted-foreground leading-7">
                        RootCX reserves the <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">rootcx_system</code> PostgreSQL schema for its internal tables. These tables store the runtime's operational state and are managed exclusively by the runtime. You should not write to these tables directly — use the API instead.
                    </p>

                    <div className="overflow-x-auto rounded-lg border border-border">
                        <table className="w-full text-sm">
                            <thead>
                                <tr className="border-b border-border bg-[#0d0d0d]">
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Table</th>
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Purpose</th>
                                </tr>
                            </thead>
                            <tbody className="divide-y divide-border/50">
                                {[
                                    { table: "rootcx_system.apps", purpose: "One row per deployed application. Stores the appId, name, version, and the full manifest JSON." },
                                    { table: "rootcx_system.users", purpose: "Registered user accounts. Stores email, hashed password (argon2), display name, and verification state." },
                                    { table: "rootcx_system.sessions", purpose: "Active authentication sessions. Each row is a JWT-backed session with an expiry timestamp." },
                                    { table: "rootcx_system.rbac_roles", purpose: "Role definitions. Each row is a named role with an optional inherits array and a reference to the app." },
                                    { table: "rootcx_system.rbac_assignments", purpose: "User-to-role assignments. Many-to-many join table between users and roles." },
                                    { table: "rootcx_system.rbac_policies", purpose: "Access control policies. Stores role, entity, allowed actions, and the ownerOnly flag." },
                                    { table: "rootcx_system.secrets", purpose: "Encrypted application secrets. Values are stored as AES-256-GCM ciphertext." },
                                    { table: "rootcx_system.workers", purpose: "Registered worker scripts. Stores worker name, source code, and runtime configuration." },
                                    { table: "rootcx_system.jobs", purpose: "Job queue. Stores pending, running, and completed job records with payloads and results." },
                                    { table: "rootcx_system.scheduled_jobs", purpose: "Cron schedule definitions. Each row maps a cron expression to a worker and optional payload." },
                                    { table: "rootcx_system.audit_log", purpose: "Immutable audit trail. Records every API operation with actor, action, entity, record ID, and timestamp." },
                                ].map(({ table, purpose }) => (
                                    <tr key={table} className="hover:bg-white/[0.02] transition-colors">
                                        <td className="px-4 py-3 font-mono text-xs text-primary whitespace-nowrap">{table}</td>
                                        <td className="px-4 py-3 text-muted-foreground text-sm leading-relaxed">{purpose}</td>
                                    </tr>
                                ))}
                            </tbody>
                        </table>
                    </div>

                    <Callout variant="warning" title="Do not modify system tables directly">
                        Writing directly to <code>rootcx_system</code> tables can corrupt the runtime's internal state. Role assignments, secrets, and audit records should always be managed through the API endpoints or the Studio IDE.
                    </Callout>
                </section>

                {/* HTTP Server */}
                <section className="flex flex-col gap-4" id="http-server">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">HTTP API Server</h2>
                    <p className="text-muted-foreground leading-7">
                        The runtime serves all API traffic through a single <strong className="text-foreground font-medium">Axum</strong>-based HTTP server. Axum is a Rust web framework built on Tokio and Hyper, providing async-first request handling with minimal overhead. All routes are registered at boot time and are available for the lifetime of the process.
                    </p>

                    <PropertiesTable properties={[
                        {
                            name: "Framework",
                            type: "Axum (Rust)",
                            required: false,
                            description: "Async HTTP framework built on Tokio and Tower middleware. Provides type-safe routing, extractors, and middleware composition.",
                        },
                        {
                            name: "Default port",
                            type: "9100",
                            required: false,
                            description: "The HTTP server binds to 0.0.0.0:9100 by default. Override with the ROOTCX_PORT environment variable or the port field in rootcx.toml.",
                        },
                        {
                            name: "CORS",
                            type: "Configurable",
                            required: false,
                            description: "CORS headers are applied to all routes. Allowed origins are configured via ROOTCX_CORS_ORIGINS (comma-separated). Defaults to * in development mode.",
                        },
                        {
                            name: "Upload limit",
                            type: "50 MB",
                            required: false,
                            description: "The maximum request body size is 50 MB. Requests exceeding this limit are rejected with a 413 Payload Too Large response. Configurable via ROOTCX_MAX_BODY_SIZE.",
                        },
                        {
                            name: "TLS",
                            type: "Terminated upstream",
                            required: false,
                            description: "The runtime does not terminate TLS directly. In production, place a reverse proxy (nginx, Caddy, or a cloud load balancer) in front of the runtime to handle TLS termination.",
                        },
                    ]} />

                    <p className="text-muted-foreground leading-7 mt-2">The HTTP server exposes the following route groups:</p>

                    <div className="overflow-x-auto rounded-lg border border-border">
                        <table className="w-full text-sm">
                            <thead>
                                <tr className="border-b border-border bg-[#0d0d0d]">
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Route Group</th>
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Base Path</th>
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Description</th>
                                </tr>
                            </thead>
                            <tbody className="divide-y divide-border/50">
                                {[
                                    { group: "Status & Health", path: "/api/v1/status, /health", desc: "Runtime status and liveness checks. No authentication required." },
                                    { group: "Authentication", path: "/api/v1/auth/*", desc: "Registration, login, logout, token refresh, and password management." },
                                    { group: "Data API", path: "/api/v1/data/:entity/*", desc: "CRUD operations for all entities defined in the data contract. RBAC-enforced." },
                                    { group: "RBAC", path: "/api/v1/rbac/*", desc: "Role assignment, revocation, user role listing, and permission querying." },
                                    { group: "Workers", path: "/api/v1/workers/*", desc: "Worker registration, invocation (RPC), and management." },
                                    { group: "Jobs", path: "/api/v1/jobs/*", desc: "Job enqueue, status polling, and result retrieval." },
                                    { group: "Secrets", path: "/api/v1/secrets/*", desc: "Secret creation, retrieval (by name), update, and deletion." },
                                    { group: "Audit Logs", path: "/api/v1/audit/*", desc: "Paginated audit log queries with filtering by actor, action, and entity." },
                                    { group: "Manifest", path: "/api/v1/manifest", desc: "GET the current deployed manifest. POST to deploy a new manifest version." },
                                ].map(({ group, path, desc }) => (
                                    <tr key={group} className="hover:bg-white/[0.02] transition-colors">
                                        <td className="px-4 py-3 text-sm font-medium text-foreground whitespace-nowrap">{group}</td>
                                        <td className="px-4 py-3 font-mono text-xs text-primary whitespace-nowrap">{path}</td>
                                        <td className="px-4 py-3 text-muted-foreground text-sm leading-relaxed">{desc}</td>
                                    </tr>
                                ))}
                            </tbody>
                        </table>
                    </div>
                </section>

                {/* Connection Pool */}
                <section className="flex flex-col gap-4" id="connection-pool">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Connection Pool</h2>
                    <p className="text-muted-foreground leading-7">
                        All database operations go through a single <strong className="text-foreground font-medium">SQLx <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">PgPool</code></strong>. The pool is shared across all Axum handlers, worker threads, and background jobs via an <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">Arc</code>-wrapped reference. This ensures that the total number of open database connections is bounded and predictable regardless of request concurrency.
                    </p>

                    <PropertiesTable properties={[
                        {
                            name: "min_connections",
                            type: "u32",
                            required: false,
                            default: "2",
                            description: "Minimum number of connections kept alive in the pool at all times. Set via ROOTCX_DB_MIN_CONNECTIONS.",
                        },
                        {
                            name: "max_connections",
                            type: "u32",
                            required: false,
                            default: "20",
                            description: "Maximum number of concurrent connections. Requests that need a connection when the pool is exhausted wait up to the acquire_timeout. Set via ROOTCX_DB_MAX_CONNECTIONS.",
                        },
                        {
                            name: "acquire_timeout",
                            type: "Duration",
                            required: false,
                            default: "30s",
                            description: "Maximum time to wait for a connection from the pool before returning a 503 Service Unavailable. Set via ROOTCX_DB_ACQUIRE_TIMEOUT.",
                        },
                        {
                            name: "idle_timeout",
                            type: "Duration",
                            required: false,
                            default: "600s",
                            description: "Connections idle for longer than this duration are closed and removed from the pool.",
                        },
                        {
                            name: "Statement cache",
                            type: "Enabled",
                            required: false,
                            description: "SQLx caches prepared statements per connection. The cache is invalidated automatically after schema changes (column type migrations) to prevent stale plan errors.",
                        },
                    ]} />

                    <Callout variant="tip" title="Tuning for production">
                        For production deployments, set <code>ROOTCX_DB_MAX_CONNECTIONS</code> to approximately <code>60-80% of PostgreSQL's max_connections</code> setting, leaving headroom for administrative connections. Monitor pool wait times via the <code>/api/v1/status</code> endpoint's <code>db.pool_size</code> and <code>db.idle_connections</code> fields.
                    </Callout>
                </section>

                {/* Status Endpoint */}
                <section className="flex flex-col gap-4" id="status-endpoint">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Status Endpoint</h2>
                    <p className="text-muted-foreground leading-7">
                        The <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">GET /api/v1/status</code> endpoint returns a JSON document describing the current state of the runtime and all its subsystems. It does not require authentication. This endpoint is suitable for use as a readiness probe in container orchestration systems.
                    </p>

                    <CodeBlock language="bash" code={`curl http://localhost:9100/api/v1/status`} />

                    <CodeBlock language="json" filename="Response" code={`{
  "status": "ok",
  "version": "1.0.0",
  "uptime_seconds": 3847,
  "app": {
    "id": "crm-app",
    "name": "CRM Application",
    "version": "1.0.0",
    "deployed_at": "2024-12-01T10:00:00Z"
  },
  "db": {
    "status": "connected",
    "pool_size": 20,
    "idle_connections": 18,
    "active_connections": 2,
    "acquire_timeout_ms": 30000
  },
  "worker_manager": {
    "status": "running",
    "worker_count": 4,
    "workers": [
      { "name": "send-email",      "status": "ready" },
      { "name": "sync-crm",        "status": "ready" },
      { "name": "generate-report", "status": "ready" },
      { "name": "webhook-handler", "status": "ready" }
    ]
  },
  "scheduler": {
    "status": "running",
    "job_count": 2,
    "next_run": "2024-12-01T11:00:00Z"
  },
  "secret_manager": {
    "status": "ready",
    "secret_count": 7
  }
}`} />

                    <PropertiesTable properties={[
                        {
                            name: "status",
                            type: "\"ok\" | \"degraded\" | \"error\"",
                            required: false,
                            description: "Overall runtime health. \"degraded\" means one or more non-critical subsystems have issues. \"error\" means a critical subsystem (database, HTTP server) has failed.",
                        },
                        {
                            name: "version",
                            type: "string",
                            required: false,
                            description: "The RootCX runtime version.",
                        },
                        {
                            name: "uptime_seconds",
                            type: "number",
                            required: false,
                            description: "Seconds elapsed since the runtime completed the boot sequence.",
                        },
                        {
                            name: "app",
                            type: "object",
                            required: false,
                            description: "The currently deployed application's ID, name, manifest version, and deployment timestamp.",
                        },
                        {
                            name: "db",
                            type: "object",
                            required: false,
                            description: "Connection pool statistics: pool_size, idle_connections, active_connections, and acquire_timeout.",
                        },
                        {
                            name: "worker_manager",
                            type: "object",
                            required: false,
                            description: "Worker manager status and per-worker readiness information.",
                        },
                        {
                            name: "scheduler",
                            type: "object",
                            required: false,
                            description: "Scheduler status, number of scheduled jobs, and next scheduled run time.",
                        },
                    ]} />
                </section>

                {/* Health Check */}
                <section className="flex flex-col gap-4" id="health-endpoint">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Health Check</h2>
                    <p className="text-muted-foreground leading-7">
                        The <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">GET /health</code> endpoint is a minimal liveness check designed for load balancers and container orchestration health probes. It returns <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">200 OK</code> with the body <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">ok</code> if the HTTP server is running and the database is reachable. It returns <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">503 Service Unavailable</code> otherwise.
                    </p>

                    <CodeBlock language="bash" code={`# Liveness probe
curl -f http://localhost:9100/health

# Response on success: 200 OK
# Body: ok

# Response on failure: 503 Service Unavailable
# Body: error`} />

                    <Callout variant="info" title="Use /health for container probes">
                        Configure your Docker or Kubernetes liveness/readiness probes to hit <code>/health</code> rather than <code>/api/v1/status</code>. The health endpoint is intentionally lightweight — it does a single <code>SELECT 1</code> to confirm database connectivity and returns immediately, with no JSON serialization overhead.
                    </Callout>
                </section>

                {/* Platform Support */}
                <section className="flex flex-col gap-4" id="platform-support">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Platform Support</h2>
                    <p className="text-muted-foreground leading-7">
                        The RootCX runtime is distributed as a single statically-linked binary. Each release includes builds for the following platforms:
                    </p>

                    <div className="overflow-x-auto rounded-lg border border-border">
                        <table className="w-full text-sm">
                            <thead>
                                <tr className="border-b border-border bg-[#0d0d0d]">
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Platform</th>
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Architecture</th>
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Binary Name</th>
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Support Tier</th>
                                </tr>
                            </thead>
                            <tbody className="divide-y divide-border/50">
                                {[
                                    { platform: "macOS", arch: "Apple Silicon (arm64)", bin: "rootcx-darwin-arm64", tier: "Primary" },
                                    { platform: "macOS", arch: "Intel (x86_64)", bin: "rootcx-darwin-x86_64", tier: "Primary" },
                                    { platform: "Linux", arch: "x86_64 (glibc)", bin: "rootcx-linux-x86_64", tier: "Primary" },
                                    { platform: "Linux", arch: "arm64 (glibc)", bin: "rootcx-linux-arm64", tier: "Primary" },
                                    { platform: "Windows", arch: "x86_64", bin: "rootcx-windows-x86_64.exe", tier: "Beta" },
                                ].map(({ platform, arch, bin, tier }) => (
                                    <tr key={bin} className="hover:bg-white/[0.02] transition-colors">
                                        <td className="px-4 py-3 text-sm font-medium text-foreground">{platform}</td>
                                        <td className="px-4 py-3 text-sm text-muted-foreground">{arch}</td>
                                        <td className="px-4 py-3 font-mono text-xs text-primary">{bin}</td>
                                        <td className="px-4 py-3">
                                            <span className={`text-xs px-2 py-0.5 rounded-full ${tier === "Primary" ? "bg-green-500/10 text-green-400" : "bg-yellow-500/10 text-yellow-400"}`}>
                                                {tier}
                                            </span>
                                        </td>
                                    </tr>
                                ))}
                            </tbody>
                        </table>
                    </div>

                    <Callout variant="note" title="Docker images">
                        Official Docker images are available on the GitHub Container Registry at <code>ghcr.io/rootcx/runtime:latest</code>. The image is based on Debian slim and includes the runtime binary and bundled PostgreSQL. Use the Docker image for production deployments behind a container orchestrator.
                    </Callout>
                </section>

                {/* Related */}
                <section className="flex flex-col gap-4" id="related">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Related Pages</h2>
                    <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
                        {[
                            { href: "/concepts/manifest", title: "App Manifest", desc: "The JSON document the runtime reads to provision your application's data model and RBAC." },
                            { href: "/self-hosting/config", title: "Configuration", desc: "All environment variables and config file options for the runtime." },
                            { href: "/modules/workers", title: "Workers & RPC", desc: "How the Worker Manager loads and dispatches JavaScript worker scripts." },
                            { href: "/modules/jobs", title: "Job Queue", desc: "How the Scheduler and job queue interact with the Worker Manager." },
                        ].map(({ href, title, desc }) => (
                            <Link key={href} href={href} className="group rounded-lg border border-border bg-white/[0.02] p-4 transition-colors hover:border-border/80 hover:bg-white/[0.04]">
                                <div className="flex items-center justify-between">
                                    <p className="text-sm font-medium text-foreground">{title}</p>
                                    <ChevronRight className="h-4 w-4 text-muted-foreground transition-transform group-hover:translate-x-0.5" />
                                </div>
                                <p className="mt-1 text-xs text-muted-foreground leading-relaxed">{desc}</p>
                            </Link>
                        ))}
                    </div>
                </section>

                <PageNav href="/concepts/runtime" />
            </div>
        </DocsLayout>
    );
}
