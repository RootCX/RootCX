import { DocsLayout } from "@/components/layout/DocsLayout";
import { PageNav } from "@/components/docs/PageNav";
import { CodeBlock } from "@/components/docs/CodeBlock";
import { Callout } from "@/components/docs/Callout";
import { PropertiesTable } from "@/components/docs/PropertiesTable";
import { ChevronRight } from "lucide-react";
import Link from "next/link";

const toc = [
    { id: "overview", title: "Overview" },
    { id: "environment-variables", title: "Environment variables" },
    { id: "auth-config", title: "Auth configuration" },
    { id: "jwt-config", title: "JWT configuration" },
    { id: "encryption-config", title: "Encryption configuration" },
    { id: "database-config", title: "Database configuration" },
    { id: "logging-config", title: "Logging" },
    { id: "key-files", title: "Key files" },
    { id: "example-env", title: "Example .env" },
];

export default function SelfHostingConfig() {
    return (
        <DocsLayout toc={toc}>
            <div className="flex flex-col gap-10">

                <div className="flex items-center gap-1.5 text-xs text-muted-foreground font-medium">
                    <Link href="/" className="hover:text-foreground transition-colors">RootCX</Link>
                    <ChevronRight className="h-3 w-3" />
                    <Link href="/self-hosting" className="hover:text-foreground transition-colors">Self-Hosting</Link>
                    <ChevronRight className="h-3 w-3" />
                    <span className="text-foreground">Configuration</span>
                </div>

                <header className="flex flex-col gap-4">
                    <h1 className="text-4xl font-semibold tracking-tight lg:text-5xl">Configuration</h1>
                    <p className="text-lg text-muted-foreground max-w-2xl leading-relaxed">
                        Complete reference for all environment variables and configuration options available in RootCX Core.
                    </p>
                </header>

                {/* Overview */}
                <section className="flex flex-col gap-4" id="overview">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Overview</h2>
                    <p className="text-muted-foreground leading-7">
                        RootCX Core is configured primarily through <strong className="text-foreground font-medium">environment variables</strong>. Sensible defaults are applied for all settings — on a fresh install the daemon is fully operational without any configuration.
                    </p>
                    <p className="text-muted-foreground leading-7">
                        Cryptographic keys (JWT signing key and AES master key) are auto-generated on first boot and stored as files in the data directory. They can be overridden with environment variables for environments where secrets are managed externally (e.g., Kubernetes Secrets, Vault).
                    </p>
                    <Callout variant="tip" title="Zero-config by default">
                        For local development, no configuration is needed. All defaults are designed to work out of the box. For production deployments, review the auth mode and key management settings below.
                    </Callout>
                </section>

                {/* Environment variables */}
                <section className="flex flex-col gap-4" id="environment-variables">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Environment variables</h2>
                    <p className="text-muted-foreground leading-7">
                        All environment variables are prefixed with <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">ROOTCX_</code> (except <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">RUST_LOG</code>). They can be set in the shell environment or in a <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">.env</code> file in the working directory.
                    </p>
                    <PropertiesTable properties={[
                        {
                            name: "ROOTCX_AUTH",
                            type: "string",
                            required: false,
                            default: "public",
                            description: "Authentication enforcement mode. Set to 'required' to reject all unauthenticated requests. Default 'public' validates tokens if present but allows unauthenticated access.",
                        },
                        {
                            name: "ROOTCX_JWT_SECRET",
                            type: "string (hex)",
                            required: false,
                            description: "32-byte JWT signing secret as a hex string (64 hex characters). If not set, the daemon reads from config/jwt.key or generates one on first boot.",
                        },
                        {
                            name: "ROOTCX_MASTER_KEY",
                            type: "string (hex)",
                            required: false,
                            description: "32-byte AES-256-GCM master key as a hex string (64 hex characters). If not set, the daemon reads from config/master.key or generates one on first boot.",
                        },
                        {
                            name: "ROOTCX_DATA_DIR",
                            type: "string (path)",
                            required: false,
                            default: "~/RootCX",
                            description: "Override the root data directory. All runtime data (PostgreSQL, app code, keys) is stored here.",
                        },
                        {
                            name: "ROOTCX_PORT",
                            type: "number",
                            required: false,
                            default: "9100",
                            description: "HTTP server port for the Core API.",
                        },
                        {
                            name: "ROOTCX_PG_PORT",
                            type: "number",
                            required: false,
                            default: "5480",
                            description: "Port for the embedded PostgreSQL instance.",
                        },
                        {
                            name: "ROOTCX_LOG_LEVEL",
                            type: "string",
                            required: false,
                            default: "info",
                            description: "Daemon log level. Accepted values: trace, debug, info, warn, error.",
                        },
                        {
                            name: "RUST_LOG",
                            type: "string",
                            required: false,
                            description: "Fine-grained Rust log filter (e.g., 'rootcx_core=debug,sqlx=warn'). Overrides ROOTCX_LOG_LEVEL when set.",
                        },
                    ]} />
                </section>

                {/* Auth config */}
                <section className="flex flex-col gap-4" id="auth-config">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Auth configuration</h2>
                    <p className="text-muted-foreground leading-7">
                        The <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">ROOTCX_AUTH</code> variable controls how the runtime handles unauthenticated requests.
                    </p>
                    <div className="rounded-lg border border-border overflow-hidden mt-2">
                        <table className="w-full text-sm">
                            <thead>
                                <tr className="border-b border-border bg-[#0d0d0d]">
                                    <th className="px-4 py-3 text-left font-semibold">Mode</th>
                                    <th className="px-4 py-3 text-left font-semibold">Behavior</th>
                                    <th className="px-4 py-3 text-left font-semibold">Use case</th>
                                </tr>
                            </thead>
                            <tbody>
                                <tr className="border-b border-border/50">
                                    <td className="px-4 py-3"><code className="font-mono text-xs text-primary">public</code></td>
                                    <td className="px-4 py-3 text-muted-foreground text-xs">Tokens validated if provided; requests without tokens proceed as anonymous. RBAC policies still enforced on authenticated requests.</td>
                                    <td className="px-4 py-3 text-muted-foreground text-xs">Local development, internal tools with trusted network</td>
                                </tr>
                                <tr>
                                    <td className="px-4 py-3"><code className="font-mono text-xs text-primary">required</code></td>
                                    <td className="px-4 py-3 text-muted-foreground text-xs">All requests to /api/v1/* must include a valid Bearer token. Returns 401 Unauthorized otherwise.</td>
                                    <td className="px-4 py-3 text-muted-foreground text-xs">Production deployments, customer-facing apps</td>
                                </tr>
                            </tbody>
                        </table>
                    </div>
                    <CodeBlock language="bash" code={`# Enable required auth mode
ROOTCX_AUTH=required ./rootcx-core start`} />
                    <Callout variant="info" title="Auth and CRUD">
                        In <code>public</code> mode, requests without a token still go through RBAC evaluation. If no policies grant anonymous access, they may still be rejected with <code>403 Forbidden</code>.
                    </Callout>
                </section>

                {/* JWT config */}
                <section className="flex flex-col gap-4" id="jwt-config">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">JWT configuration</h2>
                    <p className="text-muted-foreground leading-7">
                        RootCX uses HS256 (HMAC-SHA256) to sign JWT tokens. The signing key is a 32-byte secret.
                    </p>

                    <h3 className="text-lg font-semibold text-foreground mt-2">Key resolution order</h3>
                    <ol className="flex flex-col gap-2 ml-4 mt-1">
                        {[
                            { label: "ROOTCX_JWT_SECRET env var", desc: "64-character hex string" },
                            { label: "config/jwt.key file", desc: "Binary file at {dataDir}/config/jwt.key" },
                            { label: "Auto-generated on first boot", desc: "32 random bytes, written to jwt.key" },
                        ].map((item, i) => (
                            <li key={i} className="flex items-center gap-3 text-sm">
                                <span className="flex h-5 w-5 shrink-0 items-center justify-center rounded-full bg-primary/10 text-xs font-bold text-primary">{i + 1}</span>
                                <span className="font-medium text-foreground">{item.label}</span>
                                <span className="text-muted-foreground">— {item.desc}</span>
                            </li>
                        ))}
                    </ol>

                    <h3 className="text-lg font-semibold text-foreground mt-4">Token TTLs</h3>
                    <div className="rounded-lg border border-border overflow-hidden">
                        <table className="w-full text-sm">
                            <thead>
                                <tr className="border-b border-border bg-[#0d0d0d]">
                                    <th className="px-4 py-3 text-left font-semibold">Token type</th>
                                    <th className="px-4 py-3 text-left font-semibold">Expiry</th>
                                    <th className="px-4 py-3 text-left font-semibold">Claims</th>
                                </tr>
                            </thead>
                            <tbody>
                                <tr className="border-b border-border/50">
                                    <td className="px-4 py-3 font-mono text-xs text-primary">access_token</td>
                                    <td className="px-4 py-3 text-muted-foreground text-xs">15 minutes</td>
                                    <td className="px-4 py-3 text-muted-foreground text-xs">sub (userId), username, exp, iat</td>
                                </tr>
                                <tr>
                                    <td className="px-4 py-3 font-mono text-xs text-primary">refresh_token</td>
                                    <td className="px-4 py-3 text-muted-foreground text-xs">30 days</td>
                                    <td className="px-4 py-3 text-muted-foreground text-xs">sub (userId), session_id, exp, iat</td>
                                </tr>
                            </tbody>
                        </table>
                    </div>

                    <h3 className="text-lg font-semibold text-foreground mt-4">Generate a JWT secret</h3>
                    <CodeBlock language="bash" code={`# Generate a 32-byte key and hex-encode it
openssl rand -hex 32
# Example output: a3f8c2e1b4d7a9f0e2c5b8a1d4e7f0a3b6c9d2e5f8a1b4c7d0e3f6a9b2c5d8e1`} />
                    <Callout variant="warning" title="Key rotation">
                        Changing the JWT secret invalidates all existing tokens. All logged-in users will be signed out and must log in again.
                    </Callout>
                </section>

                {/* Encryption config */}
                <section className="flex flex-col gap-4" id="encryption-config">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Encryption configuration</h2>
                    <p className="text-muted-foreground leading-7">
                        The AES-256-GCM master key is used to encrypt all application secrets at rest. Each encryption uses a unique 12-byte random nonce.
                    </p>

                    <h3 className="text-lg font-semibold text-foreground mt-2">Key resolution order</h3>
                    <ol className="flex flex-col gap-2 ml-4 mt-1">
                        {[
                            { label: "ROOTCX_MASTER_KEY env var", desc: "64-character hex string" },
                            { label: "config/master.key file", desc: "Binary file at {dataDir}/config/master.key" },
                            { label: "Auto-generated on first boot", desc: "32 random bytes, written to master.key" },
                        ].map((item, i) => (
                            <li key={i} className="flex items-center gap-3 text-sm">
                                <span className="flex h-5 w-5 shrink-0 items-center justify-center rounded-full bg-primary/10 text-xs font-bold text-primary">{i + 1}</span>
                                <span className="font-medium text-foreground">{item.label}</span>
                                <span className="text-muted-foreground">— {item.desc}</span>
                            </li>
                        ))}
                    </ol>

                    <CodeBlock language="bash" code={`# Generate a master key
openssl rand -hex 32`} />
                    <Callout variant="warning" title="Back up your master key">
                        If you lose the master key, all application secrets become permanently inaccessible. Store it in a secure secrets manager and back it up separately from the data directory.
                    </Callout>
                </section>

                {/* Database config */}
                <section className="flex flex-col gap-4" id="database-config">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Database configuration</h2>
                    <p className="text-muted-foreground leading-7">
                        RootCX manages the PostgreSQL lifecycle entirely — you do not configure a connection string. The embedded PostgreSQL instance is initialized, started, and stopped by the daemon.
                    </p>
                    <div className="rounded-lg border border-border overflow-hidden">
                        <table className="w-full text-sm">
                            <thead>
                                <tr className="border-b border-border bg-[#0d0d0d]">
                                    <th className="px-4 py-3 text-left font-semibold">Setting</th>
                                    <th className="px-4 py-3 text-left font-semibold">Default</th>
                                    <th className="px-4 py-3 text-left font-semibold">Notes</th>
                                </tr>
                            </thead>
                            <tbody>
                                {[
                                    ["Port", "5480", "Configured via ROOTCX_PG_PORT"],
                                    ["Host", "127.0.0.1", "Localhost only, not network-accessible"],
                                    ["Database", "rootcx", "System database name"],
                                    ["User", "rootcx", "Internal database user"],
                                    ["Data directory", "~/RootCX/data/pg/", "PGDATA path"],
                                    ["PostgreSQL version", "18.1", "Bundled binary, not system PostgreSQL"],
                                    ["Max connections", "100", "PgPool max_connections setting"],
                                ].map(([setting, def, notes], i) => (
                                    <tr key={i} className="border-b border-border/50 last:border-0">
                                        <td className="px-4 py-3 font-mono text-xs text-foreground">{setting}</td>
                                        <td className="px-4 py-3 text-muted-foreground text-xs font-mono">{def}</td>
                                        <td className="px-4 py-3 text-muted-foreground text-xs">{notes}</td>
                                    </tr>
                                ))}
                            </tbody>
                        </table>
                    </div>
                    <Callout variant="info" title="Direct database access">
                        You can connect to the embedded PostgreSQL directly using <code>psql -h 127.0.0.1 -p 5480 -U rootcx rootcx</code> while the daemon is running. This is useful for debugging but avoid mutating system schema tables directly.
                    </Callout>
                </section>

                {/* Logging */}
                <section className="flex flex-col gap-4" id="logging-config">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Logging</h2>
                    <p className="text-muted-foreground leading-7">
                        RootCX uses the Rust <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">tracing</code> crate for structured logging. Log output goes to stdout by default.
                    </p>
                    <CodeBlock language="bash" code={`# Set log level
ROOTCX_LOG_LEVEL=debug ./rootcx-core start

# Fine-grained control with RUST_LOG (overrides ROOTCX_LOG_LEVEL)
RUST_LOG=rootcx_core=debug,sqlx=warn,axum=info ./rootcx-core start

# Log levels: trace | debug | info | warn | error`} />
                    <p className="text-muted-foreground leading-7">
                        For production, redirect stdout to a log file or use a process manager like systemd that captures stdout automatically:
                    </p>
                    <CodeBlock language="bash" code={`# Manual redirect
./rootcx-core start >> ~/RootCX/logs/runtime.log 2>&1 &

# Or set up logrotate for the log file
# /etc/logrotate.d/rootcx
/home/ubuntu/RootCX/logs/runtime.log {
    daily
    rotate 14
    compress
    missingok
    notifempty
}`} />
                </section>

                {/* Key files */}
                <section className="flex flex-col gap-4" id="key-files">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Key files</h2>
                    <p className="text-muted-foreground leading-7">
                        The following files are auto-generated in the data directory on first boot and should be backed up securely:
                    </p>
                    <div className="rounded-lg border border-border overflow-hidden">
                        <table className="w-full text-sm">
                            <thead>
                                <tr className="border-b border-border bg-[#0d0d0d]">
                                    <th className="px-4 py-3 text-left font-semibold">File</th>
                                    <th className="px-4 py-3 text-left font-semibold">Purpose</th>
                                    <th className="px-4 py-3 text-left font-semibold">Overridden by</th>
                                </tr>
                            </thead>
                            <tbody>
                                {[
                                    ["~/RootCX/config/jwt.key", "32-byte JWT signing key", "ROOTCX_JWT_SECRET"],
                                    ["~/RootCX/config/master.key", "32-byte AES master key", "ROOTCX_MASTER_KEY"],
                                ].map(([file, purpose, override], i) => (
                                    <tr key={i} className="border-b border-border/50 last:border-0">
                                        <td className="px-4 py-3 font-mono text-xs text-primary">{file}</td>
                                        <td className="px-4 py-3 text-muted-foreground text-xs">{purpose}</td>
                                        <td className="px-4 py-3 text-muted-foreground text-xs font-mono">{override}</td>
                                    </tr>
                                ))}
                            </tbody>
                        </table>
                    </div>
                    <CodeBlock language="bash" code={`# Backup your keys
cp ~/RootCX/config/jwt.key ./backups/jwt.key
cp ~/RootCX/config/master.key ./backups/master.key
chmod 600 ./backups/*.key`} />
                </section>

                {/* Example .env */}
                <section className="flex flex-col gap-4" id="example-env">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Example .env</h2>
                    <p className="text-muted-foreground leading-7">
                        A complete example configuration for a production deployment:
                    </p>
                    <CodeBlock language="bash" filename=".env" code={`# ── Authentication ──────────────────────────────────────────
# Require authentication on all API requests
ROOTCX_AUTH=required

# ── Cryptographic keys ───────────────────────────────────────
# Generate with: openssl rand -hex 32
ROOTCX_JWT_SECRET=a3f8c2e1b4d7a9f0e2c5b8a1d4e7f0a3b6c9d2e5f8a1b4c7d0e3f6a9b2c5d8e1
ROOTCX_MASTER_KEY=b7e2f5a8c1d4e7b0a3f6c9d2e5b8a1d4f7e0b3c6d9e2f5a8b1d4e7f0a3c6d9e2

# ── Ports ────────────────────────────────────────────────────
ROOTCX_PORT=9100
ROOTCX_PG_PORT=5480

# ── Data directory ───────────────────────────────────────────
ROOTCX_DATA_DIR=/opt/rootcx/data

# ── Logging ─────────────────────────────────────────────────
ROOTCX_LOG_LEVEL=info
# RUST_LOG=rootcx_core=debug,sqlx=warn`} />
                    <Callout variant="warning" title="Never commit .env to git">
                        Add <code>.env</code> to your <code>.gitignore</code>. For production, use environment-specific secret management (e.g., systemd <code>EnvironmentFile</code>, Kubernetes Secrets, or a secrets manager).
                    </Callout>
                </section>

                <PageNav href="/self-hosting/config" />
            </div>
        </DocsLayout>
    );
}
