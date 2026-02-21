import { DocsLayout } from "@/components/layout/DocsLayout";
import { PageNav } from "@/components/docs/PageNav";
import { CodeBlock, TerminalBlock } from "@/components/docs/CodeBlock";
import { Callout } from "@/components/docs/Callout";
import { ChevronRight } from "lucide-react";
import Link from "next/link";

const toc = [
    { id: "prerequisites", title: "Prerequisites" },
    { id: "install-studio", title: "Install Studio" },
    { id: "define-manifest", title: "Define a manifest" },
    { id: "install-app", title: "Install the app" },
    { id: "query-data", title: "Query data" },
    { id: "add-auth", title: "Add authentication" },
    { id: "deploy-logic", title: "Deploy custom logic" },
    { id: "whats-next", title: "What's next" },
];

export default function QuickStart() {
    return (
        <DocsLayout toc={toc}>
            <div className="flex flex-col gap-10">

                <div className="flex items-center gap-1.5 text-xs text-muted-foreground font-medium">
                    <Link href="/" className="hover:text-foreground transition-colors">RootCX</Link>
                    <ChevronRight className="h-3 w-3" />
                    <span className="text-foreground">Quick Start</span>
                </div>

                <header className="flex flex-col gap-4">
                    <div className="inline-flex items-center rounded-full border border-green-500/20 bg-green-500/5 px-3 py-1 text-xs font-medium text-green-400 w-fit">
                        5 min tutorial
                    </div>
                    <h1 className="text-4xl font-semibold tracking-tight lg:text-5xl">Quick Start</h1>
                    <p className="text-lg text-muted-foreground max-w-2xl leading-relaxed">
                        Build and run your first RootCX application — a fully functional backend with a PostgreSQL database, automatic REST API, authentication, and custom business logic — in under 5 minutes.
                    </p>
                </header>

                {/* Prerequisites */}
                <section className="flex flex-col gap-4" id="prerequisites">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Prerequisites</h2>
                    <p className="text-muted-foreground leading-7">
                        RootCX bundles everything it needs (PostgreSQL 18, Bun runtime). The only requirement is the RootCX Core binary or Studio.
                    </p>
                    <div className="rounded-lg border border-border overflow-hidden">
                        <table className="w-full text-sm">
                            <thead>
                                <tr className="border-b border-border bg-[#0d0d0d]">
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Requirement</th>
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Version</th>
                                    <th className="px-4 py-3 text-left font-semibold text-foreground">Notes</th>
                                </tr>
                            </thead>
                            <tbody>
                                {[
                                    ["macOS / Linux / Windows", "—", "RootCX Core is platform-native"],
                                    ["RootCX Core or Studio", "Latest", "Download from GitHub Releases"],
                                    ["curl or any HTTP client", "—", "For API calls in this tutorial"],
                                ].map(([req, ver, note], i) => (
                                    <tr key={i} className="border-b border-border/50 last:border-0">
                                        <td className="px-4 py-3 font-mono text-xs text-foreground">{req}</td>
                                        <td className="px-4 py-3 text-muted-foreground text-xs">{ver}</td>
                                        <td className="px-4 py-3 text-muted-foreground text-xs">{note}</td>
                                    </tr>
                                ))}
                            </tbody>
                        </table>
                    </div>
                </section>

                {/* Step 1 */}
                <section className="flex flex-col gap-4" id="install-studio">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">
                        <span className="text-primary/60 font-mono text-lg mr-2">01</span>
                        Install Studio
                    </h2>
                    <p className="text-muted-foreground leading-7">
                        We recommend downloading the RootCX Studio installer for your specific operating system:
                    </p>
                    <ul className="list-disc list-inside text-muted-foreground leading-7 space-y-1 ml-2">
                        <li><strong>Windows:</strong> Download the <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">.exe</code> installer.</li>
                        <li><strong>macOS:</strong> Download the <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">.dmg</code> image.</li>
                        <li><strong>Linux:</strong> Download the <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">.tar.gz</code> archive.</li>
                    </ul>
                    <p className="text-muted-foreground leading-7 mt-2">
                        Studio bundles the Core daemon, so no separate install is needed. Once installed and opened, the daemon starts PostgreSQL on port <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">5480</code> and the HTTP API on port <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">9100</code>. By default, Studio manages this for you.
                    </p>
                    <p className="text-muted-foreground leading-7 mt-4 border-t border-border pt-4">
                        Alternatively, to run just the daemon headlessly (e.g., on a server):
                    </p>
                    <TerminalBlock commands={[
                        "# macOS (Apple Silicon)",
                        "curl -L https://github.com/rootcx/rootcx/releases/latest/download/rootcx-core-darwin-arm64 -o rootcx-core",
                        "chmod +x rootcx-core",
                        "./rootcx-core start",
                    ]} />
                    <p className="text-muted-foreground leading-7">
                        Verify it is healthy:
                    </p>
                    <TerminalBlock commands="curl http://localhost:9100/health" />
                    <CodeBlock language="json" code={`{ "status": "ok" }`} />
                </section>

                {/* Step 2 */}
                <section className="flex flex-col gap-4" id="define-manifest">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">
                        <span className="text-primary/60 font-mono text-lg mr-2">02</span>
                        Define a manifest
                    </h2>
                    <p className="text-muted-foreground leading-7">
                        A <strong className="text-foreground font-medium">manifest</strong> is a JSON document that describes your application — its data model, roles, and permissions. Create <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">manifest.json</code>:
                    </p>
                    <CodeBlock language="json" filename="manifest.json" code={`{
  "appId": "crm",
  "name": "My CRM",
  "version": "1.0.0",
  "description": "A simple customer management app",
  "permissions": {
    "roles": {
      "admin": { "description": "Full access" },
      "sales": { "description": "Sales team member" }
    },
    "defaultRole": "sales",
    "policies": [
      {
        "role": "admin",
        "entity": "*",
        "actions": ["create", "read", "update", "delete"],
        "ownership": false
      },
      {
        "role": "sales",
        "entity": "contacts",
        "actions": ["create", "read", "update"],
        "ownership": true
      },
      {
        "role": "sales",
        "entity": "deals",
        "actions": ["create", "read", "update"],
        "ownership": true
      }
    ]
  },
  "dataContract": [
    {
      "entityName": "contacts",
      "fields": [
        { "name": "firstName", "type": "text", "required": true },
        { "name": "lastName",  "type": "text", "required": true },
        { "name": "email",     "type": "text", "required": true },
        { "name": "phone",     "type": "text" },
        { "name": "company",   "type": "text" },
        { "name": "tags",      "type": "[text]" }
      ]
    },
    {
      "entityName": "deals",
      "fields": [
        { "name": "title",     "type": "text",        "required": true },
        { "name": "amount",    "type": "number" },
        { "name": "status",    "type": "text",
          "enumValues": ["open", "won", "lost"],
          "defaultValue": "open" },
        { "name": "contactId", "type": "entity_link",
          "references": { "entity": "contacts", "field": "id" } }
      ]
    }
  ]
}`} />
                    <Callout variant="tip" title="Schema-as-code">
                        Your manifest is version-controlled alongside your application. When you update it and re-install, RootCX automatically runs a non-destructive migration — adding new columns, adjusting types, and updating constraints.
                    </Callout>
                </section>

                {/* Step 3 */}
                <section className="flex flex-col gap-4" id="install-app">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">
                        <span className="text-primary/60 font-mono text-lg mr-2">03</span>
                        Install the app
                    </h2>
                    <p className="text-muted-foreground leading-7">
                        POST the manifest to the runtime. RootCX creates a PostgreSQL schema named after your <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">appId</code>, generates all tables, indexes, and foreign keys, and applies your RBAC policies.
                    </p>
                    <TerminalBlock commands="curl -X POST http://localhost:9100/api/v1/apps \
  -H 'Content-Type: application/json' \
  -d @manifest.json" />
                    <CodeBlock language="json" code={`{
  "app_id": "crm",
  "name": "My CRM",
  "status": "installed"
}`} />
                    <p className="text-muted-foreground leading-7">
                        That's it. Your app has a live PostgreSQL schema and a full REST API — no SQL or migrations written manually.
                    </p>
                </section>

                {/* Step 4 */}
                <section className="flex flex-col gap-4" id="query-data">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">
                        <span className="text-primary/60 font-mono text-lg mr-2">04</span>
                        Query data
                    </h2>
                    <p className="text-muted-foreground leading-7">
                        The CRUD API is live immediately. Create a contact:
                    </p>
                    <TerminalBlock commands={`curl -X POST http://localhost:9100/api/v1/apps/crm/collections/contacts \\
  -H 'Content-Type: application/json' \\
  -d '{"firstName":"Alice","lastName":"Martin","email":"alice@acme.com","company":"Acme"}'`} />
                    <CodeBlock language="json" code={`{
  "id": "f47ac10b-58cc-4372-a567-0e02b2c3d479",
  "firstName": "Alice",
  "lastName": "Martin",
  "email": "alice@acme.com",
  "company": "Acme",
  "tags": null,
  "created_at": "2024-01-15T10:30:00Z",
  "updated_at": "2024-01-15T10:30:00Z"
}`} />
                    <p className="text-muted-foreground leading-7">List all contacts:</p>
                    <TerminalBlock commands="curl http://localhost:9100/api/v1/apps/crm/collections/contacts" />
                </section>

                {/* Step 5 */}
                <section className="flex flex-col gap-4" id="add-auth">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">
                        <span className="text-primary/60 font-mono text-lg mr-2">05</span>
                        Add authentication
                    </h2>
                    <p className="text-muted-foreground leading-7">
                        Register a user and obtain a JWT token. Subsequent requests include the token so permissions are enforced.
                    </p>
                    <TerminalBlock commands={`# Register
curl -X POST http://localhost:9100/api/v1/auth/register \\
  -H 'Content-Type: application/json' \\
  -d '{"username":"alice","password":"secret123"}'

# Login
curl -X POST http://localhost:9100/api/v1/auth/login \\
  -H 'Content-Type: application/json' \\
  -d '{"username":"alice","password":"secret123"}'`} />
                    <CodeBlock language="json" code={`{
  "access_token": "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9...",
  "refresh_token": "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9...",
  "user": {
    "id": "3fa85f64-5717-4562-b3fc-2c963f66afa6",
    "username": "alice",
    "display_name": null
  }
}`} />
                    <p className="text-muted-foreground leading-7">
                        Use the <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">access_token</code> as a Bearer token on subsequent API calls. The runtime will enforce RBAC policies and ownership filters automatically.
                    </p>
                    <TerminalBlock commands={`curl http://localhost:9100/api/v1/apps/crm/collections/contacts \\
  -H 'Authorization: Bearer <access_token>'`} />
                </section>

                {/* Step 6 */}
                <section className="flex flex-col gap-4" id="deploy-logic">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">
                        <span className="text-primary/60 font-mono text-lg mr-2">06</span>
                        Deploy custom logic
                    </h2>
                    <p className="text-muted-foreground leading-7">
                        To add business logic beyond CRUD, deploy a Backend — a TypeScript/JavaScript package. Create <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">backend/index.ts</code>:
                    </p>
                    <CodeBlock language="typescript" filename="backend/index.ts" code={`export async function handleRpc(
  method: string,
  params: Record<string, unknown>,
  caller?: { userId: string; username: string }
) {
  if (method === "sendWelcomeEmail") {
    const { contactId } = params;
    // Your custom logic here
    console.log(\`Sending welcome email to contact \${contactId}\`);
    return { sent: true };
  }
  throw new Error(\`Unknown method: \${method}\`);
}

export async function handleJob(payload: Record<string, unknown>) {
  console.log("Processing background job:", payload);
  return { processed: true };
}`} />
                    <p className="text-muted-foreground leading-7">Package it as a tar.gz and deploy:</p>
                    <TerminalBlock commands={`tar -czf backend.tar.gz -C backend .

curl -X POST http://localhost:9100/api/v1/apps/crm/deploy \\
  -H "Authorization: Bearer <$ADMIN_TOKEN>" \\
  --data-binary @backend.tar.gz`} />
                    <p className="text-muted-foreground leading-7">Then invoke your RPC:</p>
                    <TerminalBlock commands={`curl -X POST http://localhost:9100/api/v1/apps/crm/rpc \\
  -H 'Content-Type: application/json' \\
  -H 'Authorization: Bearer <token>' \\
  -d '{"method":"sendWelcomeEmail","params":{"contactId":"..."}}'`} />
                </section>

                {/* What's next */}
                <section className="flex flex-col gap-4" id="whats-next">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">
                        What's next
                    </h2>
                    <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
                        {[
                            { href: "/architecture", title: "Architecture", desc: "Understand how Core, Studio, and Backends fit together." },
                            { href: "/concepts/manifest", title: "App Manifest", desc: "Deep-dive into all manifest options and field types." },
                            { href: "/modules/authentication", title: "Authentication", desc: "Configure auth modes, JWT settings, and user management." },
                            { href: "/modules/backend", title: "Backend & RPC", desc: "Learn the full backend lifecycle, IPC protocol, and job handling." },
                        ].map((item, i) => (
                            <Link key={i} href={item.href} className="group flex flex-col gap-1.5 rounded-lg border border-border bg-[#111] hover:bg-[#141414] hover:border-primary/40 transition-all p-4">
                                <span className="font-medium text-foreground group-hover:text-primary transition-colors text-sm">{item.title} →</span>
                                <span className="text-xs text-muted-foreground leading-relaxed">{item.desc}</span>
                            </Link>
                        ))}
                    </div>
                </section>

                <PageNav href="/quickstart" />
            </div>
        </DocsLayout>
    );
}
