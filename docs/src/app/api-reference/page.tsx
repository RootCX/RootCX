import { DocsLayout } from "@/components/layout/DocsLayout";
import { PageNav } from "@/components/docs/PageNav";
import { CodeBlock } from "@/components/docs/CodeBlock";
import { Callout } from "@/components/docs/Callout";
import { ChevronRight } from "lucide-react";
import Link from "next/link";

const toc = [
    { id: "overview", title: "Overview" },
    { id: "authentication", title: "Authentication" },
    { id: "system-routes", title: "System" },
    { id: "auth-routes", title: "Auth" },
    { id: "apps-routes", title: "Apps" },
    { id: "crud-routes", title: "Data (CRUD)" },
    { id: "worker-routes", title: "Workers" },
    { id: "job-routes", title: "Jobs" },
    { id: "secret-routes", title: "Secrets" },
    { id: "rbac-routes", title: "RBAC" },
    { id: "audit-routes", title: "Audit" },
    { id: "error-format", title: "Error format" },
];

function Badge({ method }: { method: "GET" | "POST" | "PATCH" | "DELETE" | "SSE" }) {
    const styles: Record<string, string> = {
        GET: "bg-green-500/10 text-green-400 border-green-500/20",
        POST: "bg-blue-500/10 text-blue-400 border-blue-500/20",
        PATCH: "bg-yellow-500/10 text-yellow-400 border-yellow-500/20",
        DELETE: "bg-red-500/10 text-red-400 border-red-500/20",
        SSE: "bg-purple-500/10 text-purple-400 border-purple-500/20",
    };
    return (
        <span className={`inline-flex items-center rounded px-1.5 py-0.5 text-[10px] font-bold font-mono border shrink-0 ${styles[method]}`}>
            {method}
        </span>
    );
}

function Route({ method, path, desc }: { method: "GET" | "POST" | "PATCH" | "DELETE" | "SSE"; path: string; desc: string }) {
    return (
        <div className="flex items-center gap-3 py-3 border-b border-border/50 last:border-0">
            <Badge method={method} />
            <code className="font-mono text-sm text-foreground flex-1 min-w-0 truncate">{path}</code>
            <span className="text-xs text-muted-foreground hidden sm:block">{desc}</span>
        </div>
    );
}

export default function ApiReference() {
    return (
        <DocsLayout toc={toc}>
            <div className="flex flex-col gap-10">

                <div className="flex items-center gap-1.5 text-xs text-muted-foreground font-medium">
                    <Link href="/" className="hover:text-foreground transition-colors">RootCX</Link>
                    <ChevronRight className="h-3 w-3" />
                    <span className="text-foreground">REST API Reference</span>
                </div>

                <header className="flex flex-col gap-4">
                    <h1 className="text-4xl font-semibold tracking-tight lg:text-5xl">REST API Reference</h1>
                    <p className="text-lg text-muted-foreground max-w-2xl leading-relaxed">
                        Complete reference for the RootCX Core HTTP API. All endpoints are served from <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-sm text-foreground">http://localhost:9100</code> by default.
                    </p>
                </header>

                <section className="flex flex-col gap-4" id="overview">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Overview</h2>
                    <div className="grid grid-cols-2 sm:grid-cols-4 gap-3">
                        {[
                            { label: "Base URL", value: "http://localhost:9100" },
                            { label: "Protocol", value: "HTTP/1.1" },
                            { label: "Format", value: "JSON (UTF-8)" },
                            { label: "Max body", value: "50 MB" },
                        ].map((item, i) => (
                            <div key={i} className="rounded-lg border border-border bg-[#111] p-3">
                                <p className="text-xs text-muted-foreground">{item.label}</p>
                                <p className="font-mono text-xs text-foreground mt-1">{item.value}</p>
                            </div>
                        ))}
                    </div>
                    <p className="text-muted-foreground leading-7">
                        All <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">/api/v1/*</code> routes accept and return JSON. The deploy endpoint accepts <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">application/octet-stream</code>. The logs endpoint returns <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">text/event-stream</code>.
                    </p>
                </section>

                <section className="flex flex-col gap-4" id="authentication">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Authentication</h2>
                    <p className="text-muted-foreground leading-7">
                        Pass a JWT access token as a Bearer token in the <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">Authorization</code> header:
                    </p>
                    <CodeBlock language="bash" code={`Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9...`} />
                    <p className="text-muted-foreground leading-7">
                        In <strong className="text-foreground font-medium">public</strong> mode (default), the token is optional. In <strong className="text-foreground font-medium">required</strong> mode (<code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">ROOTCX_AUTH=required</code>), all <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">/api/v1/*</code> endpoints require a valid token.
                    </p>
                </section>

                {/* System routes */}
                <section className="flex flex-col gap-4" id="system-routes">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">System</h2>
                    <div className="rounded-lg border border-border overflow-hidden divide-y divide-border/50">
                        <Route method="GET" path="/health" desc="Health check — returns {status:'ok'}" />
                        <Route method="GET" path="/api/v1/status" desc="Runtime status (postgres, runtime, forge)" />
                    </div>

                    <h3 className="text-lg font-semibold text-foreground mt-2">GET /api/v1/status</h3>
                    <CodeBlock language="json" code={`{
  "postgres": "running",
  "runtime": "ok",
  "forge": {
    "app_id": "crm",
    "status": "running"
  }
}`} />
                </section>

                {/* Auth routes */}
                <section className="flex flex-col gap-4" id="auth-routes">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Auth</h2>
                    <div className="rounded-lg border border-border overflow-hidden">
                        <Route method="POST" path="/api/v1/auth/register" desc="Register a new user" />
                        <Route method="POST" path="/api/v1/auth/login" desc="Login and receive tokens" />
                        <Route method="POST" path="/api/v1/auth/refresh" desc="Refresh an expired access token" />
                        <Route method="POST" path="/api/v1/auth/logout" desc="Invalidate session" />
                        <Route method="GET" path="/api/v1/auth/me" desc="Get current authenticated user" />
                    </div>

                    <h3 className="text-lg font-semibold text-foreground mt-2">POST /api/v1/auth/register</h3>
                    <CodeBlock language="json" code={`// Request
{
  "username": "alice",
  "password": "secret123",
  "email": "alice@example.com",    // optional
  "displayName": "Alice Martin"    // optional
}

// Response 201
{
  "user": {
    "id": "3fa85f64-5717-4562-b3fc-2c963f66afa6",
    "username": "alice",
    "email": "alice@example.com",
    "displayName": "Alice Martin"
  },
  "access_token": "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9...",
  "refresh_token": "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9..."
}`} />

                    <h3 className="text-lg font-semibold text-foreground mt-2">POST /api/v1/auth/login</h3>
                    <CodeBlock language="json" code={`// Request
{ "username": "alice", "password": "secret123" }

// Response 200
{
  "user": { "id": "...", "username": "alice", "email": "alice@example.com" },
  "access_token": "eyJ...",
  "refresh_token": "eyJ..."
}`} />

                    <h3 className="text-lg font-semibold text-foreground mt-2">POST /api/v1/auth/refresh</h3>
                    <CodeBlock language="json" code={`// Request
{ "refresh_token": "eyJ..." }

// Response 200
{ "access_token": "eyJ..." }`} />
                </section>

                {/* Apps routes */}
                <section className="flex flex-col gap-4" id="apps-routes">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Apps</h2>
                    <div className="rounded-lg border border-border overflow-hidden">
                        <Route method="GET" path="/api/v1/apps" desc="List all installed applications" />
                        <Route method="POST" path="/api/v1/apps" desc="Install an application from a manifest" />
                        <Route method="DELETE" path="/api/v1/apps/{appId}" desc="Uninstall an application" />
                    </div>

                    <h3 className="text-lg font-semibold text-foreground mt-2">POST /api/v1/apps — Install app</h3>
                    <CodeBlock language="json" code={`// Request body: AppManifest object
{
  "appId": "crm",
  "name": "My CRM",
  "version": "1.0.0",
  "dataContract": [...],
  "permissions": {...}
}

// Response 200
{
  "app_id": "crm",
  "name": "My CRM",
  "status": "installed"
}`} />
                    <Callout variant="info" title="Idempotent install">
                        Posting a manifest for an existing app ID triggers a schema sync — new tables and columns are added, changed types are migrated, and RBAC policies are updated. Existing data is preserved.
                    </Callout>
                </section>

                {/* CRUD routes */}
                <section className="flex flex-col gap-4" id="crud-routes">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Data (CRUD)</h2>
                    <div className="rounded-lg border border-border overflow-hidden">
                        <Route method="GET" path="/api/v1/apps/{appId}/collections/{entity}" desc="List all records" />
                        <Route method="POST" path="/api/v1/apps/{appId}/collections/{entity}" desc="Create a record" />
                        <Route method="GET" path="/api/v1/apps/{appId}/collections/{entity}/{id}" desc="Get a record by ID" />
                        <Route method="PATCH" path="/api/v1/apps/{appId}/collections/{entity}/{id}" desc="Partially update a record" />
                        <Route method="DELETE" path="/api/v1/apps/{appId}/collections/{entity}/{id}" desc="Delete a record" />
                    </div>
                    <h3 className="text-lg font-semibold text-foreground mt-2">Record shape</h3>
                    <CodeBlock language="json" code={`{
  "id": "f47ac10b-58cc-4372-a567-0e02b2c3d479",
  "firstName": "Alice",
  "lastName": "Martin",
  "email": "alice@acme.com",
  "tags": ["vip", "enterprise"],
  "owner_id": "3fa85f64-5717-4562-b3fc-2c963f66afa6",
  "created_at": "2024-01-15T10:30:00Z",
  "updated_at": "2024-01-15T10:30:00Z"
}`} />
                </section>

                {/* Worker routes */}
                <section className="flex flex-col gap-4" id="worker-routes">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Workers</h2>
                    <div className="rounded-lg border border-border overflow-hidden">
                        <Route method="POST" path="/api/v1/apps/{appId}/deploy" desc="Deploy worker code (tar.gz body)" />
                        <Route method="POST" path="/api/v1/apps/{appId}/worker/start" desc="Start the worker process" />
                        <Route method="POST" path="/api/v1/apps/{appId}/worker/stop" desc="Stop the worker process" />
                        <Route method="GET" path="/api/v1/apps/{appId}/worker/status" desc="Get worker status" />
                        <Route method="POST" path="/api/v1/apps/{appId}/rpc" desc="Invoke a worker RPC method" />
                        <Route method="SSE" path="/api/v1/apps/{appId}/logs" desc="Stream live worker logs" />
                    </div>

                    <h3 className="text-lg font-semibold text-foreground mt-2">POST /api/v1/apps/{"{appId}"}/rpc</h3>
                    <CodeBlock language="json" code={`// Request
{
  "method": "sendWelcomeEmail",
  "params": { "contactId": "f47ac10b..." }
}

// Response 200
{ "result": { "sent": true } }

// Error response
{ "error": "Failed to connect to SMTP server" }`} />

                    <h3 className="text-lg font-semibold text-foreground mt-2">GET /api/v1/apps/{"{appId}"}/worker/status</h3>
                    <CodeBlock language="json" code={`{
  "status": "running",
  "pid": 12345,
  "uptime_seconds": 3600,
  "restarts": 2
}`} />
                </section>

                {/* Job routes */}
                <section className="flex flex-col gap-4" id="job-routes">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Jobs</h2>
                    <div className="rounded-lg border border-border overflow-hidden">
                        <Route method="POST" path="/api/v1/apps/{appId}/jobs" desc="Enqueue a background job" />
                        <Route method="GET" path="/api/v1/apps/{appId}/jobs" desc="List jobs (filter by status)" />
                        <Route method="GET" path="/api/v1/apps/{appId}/jobs/{jobId}" desc="Get job by ID" />
                    </div>

                    <h3 className="text-lg font-semibold text-foreground mt-2">POST /api/v1/apps/{"{appId}"}/jobs</h3>
                    <CodeBlock language="json" code={`// Request
{
  "payload": { "type": "send_report", "userId": "..." },
  "runAt": "2024-01-15T18:00:00Z"  // optional, defaults to now
}

// Response 200
{ "job_id": "a1b2c3d4-..." }

// Job object (from GET)
{
  "id": "a1b2c3d4-...",
  "status": "completed",
  "payload": { "type": "send_report", "userId": "..." },
  "result": { "sent": true },
  "error": null,
  "attempts": 1,
  "run_at": "2024-01-15T18:00:00Z",
  "created_at": "2024-01-15T10:00:00Z",
  "updated_at": "2024-01-15T18:00:05Z"
}`} />
                </section>

                {/* Secret routes */}
                <section className="flex flex-col gap-4" id="secret-routes">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Secrets</h2>
                    <div className="rounded-lg border border-border overflow-hidden">
                        <Route method="POST" path="/api/v1/apps/{appId}/secrets" desc="Set a secret (idempotent)" />
                        <Route method="GET" path="/api/v1/apps/{appId}/secrets" desc="List secret key names (not values)" />
                        <Route method="DELETE" path="/api/v1/apps/{appId}/secrets/{key}" desc="Delete a secret" />
                    </div>

                    <CodeBlock language="json" code={`// POST /api/v1/apps/crm/secrets
// Request
{ "key": "OPENAI_API_KEY", "value": "sk-..." }
// Response
{ "message": "secret 'OPENAI_API_KEY' set" }

// GET /api/v1/apps/crm/secrets
// Response — key names only, never values
["OPENAI_API_KEY", "STRIPE_SECRET_KEY", "SMTP_PASSWORD"]`} />
                </section>

                {/* RBAC routes */}
                <section className="flex flex-col gap-4" id="rbac-routes">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">RBAC</h2>
                    <div className="rounded-lg border border-border overflow-hidden">
                        <Route method="GET" path="/api/v1/apps/{appId}/roles" desc="List all roles" />
                        <Route method="GET" path="/api/v1/apps/{appId}/roles/assignments" desc="List user-role assignments" />
                        <Route method="POST" path="/api/v1/apps/{appId}/roles/assign" desc="Assign a role to a user" />
                        <Route method="POST" path="/api/v1/apps/{appId}/roles/revoke" desc="Revoke a role from a user" />
                        <Route method="GET" path="/api/v1/apps/{appId}/permissions" desc="Current user's effective permissions" />
                        <Route method="GET" path="/api/v1/apps/{appId}/permissions/{userId}" desc="Specific user's permissions" />
                    </div>

                    <CodeBlock language="json" code={`// POST /api/v1/apps/crm/roles/assign
{ "userId": "3fa85f64-...", "role": "admin" }

// GET /api/v1/apps/crm/permissions
{
  "userId": "3fa85f64-...",
  "roles": ["admin"],
  "permissions": {
    "contacts": { "actions": ["create","read","update","delete"], "ownership": false },
    "deals":    { "actions": ["create","read","update","delete"], "ownership": false }
  }
}`} />
                </section>

                {/* Audit routes */}
                <section className="flex flex-col gap-4" id="audit-routes">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Audit</h2>
                    <div className="rounded-lg border border-border overflow-hidden">
                        <Route method="GET" path="/api/v1/audit" desc="Query the audit log" />
                    </div>
                    <CodeBlock language="bash" code={`# Query parameters
GET /api/v1/audit
  ?app_id=crm         # Filter by app
  &entity=contacts    # Filter by table name
  &operation=UPDATE   # Filter by INSERT|UPDATE|DELETE
  &limit=100          # Max results (default 100)`} />
                    <CodeBlock language="json" code={`[
  {
    "id": 1001,
    "table_schema": "app_crm",
    "table_name": "contacts",
    "record_id": "f47ac10b-...",
    "operation": "UPDATE",
    "old_record": { "email": "alice@old.com" },
    "new_record": { "email": "alice@acme.com" },
    "changed_at": "2024-01-15T10:30:00Z"
  }
]`} />
                </section>

                {/* Error format */}
                <section className="flex flex-col gap-4" id="error-format">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Error format</h2>
                    <p className="text-muted-foreground leading-7">
                        All errors return a JSON body with an <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">error</code> field and the appropriate HTTP status code:
                    </p>
                    <CodeBlock language="json" code={`{ "error": "human-readable error message" }`} />
                    <div className="rounded-lg border border-border overflow-hidden">
                        <table className="w-full text-sm">
                            <thead>
                                <tr className="border-b border-border bg-[#0d0d0d]">
                                    <th className="px-4 py-3 text-left font-semibold">Status</th>
                                    <th className="px-4 py-3 text-left font-semibold">Meaning</th>
                                    <th className="px-4 py-3 text-left font-semibold">Common causes</th>
                                </tr>
                            </thead>
                            <tbody>
                                {[
                                    ["400", "Bad Request", "Invalid JSON, missing required fields, validation error"],
                                    ["401", "Unauthorized", "Missing or invalid Bearer token"],
                                    ["403", "Forbidden", "Valid token but RBAC policy denies the action"],
                                    ["404", "Not Found", "Record doesn't exist or you don't have ownership access"],
                                    ["409", "Conflict", "Username already taken, duplicate app ID"],
                                    ["500", "Internal Server Error", "Unexpected database or runtime error"],
                                    ["503", "Service Unavailable", "PostgreSQL or daemon not ready yet"],
                                ].map(([status, meaning, causes], i) => (
                                    <tr key={i} className="border-b border-border/50 last:border-0">
                                        <td className="px-4 py-3 font-mono text-xs text-primary">{status}</td>
                                        <td className="px-4 py-3 text-sm text-foreground">{meaning}</td>
                                        <td className="px-4 py-3 text-xs text-muted-foreground">{causes}</td>
                                    </tr>
                                ))}
                            </tbody>
                        </table>
                    </div>
                </section>

                <PageNav href="/api-reference" />
            </div>
        </DocsLayout>
    );
}
