import { DocsLayout } from "@/components/layout/DocsLayout";
import { PageNav } from "@/components/docs/PageNav";
import { CodeBlock } from "@/components/docs/CodeBlock";
import { Callout } from "@/components/docs/Callout";
import { PropertiesTable } from "@/components/docs/PropertiesTable";
import { ChevronRight } from "lucide-react";
import Link from "next/link";

const tocItems = [
  { id: "overview", title: "Overview" },
  { id: "worker-structure", title: "Worker Structure" },
  { id: "discovery", title: "Discovery Handshake" },
  { id: "rpc-calls", title: "RPC Calls" },
  { id: "job-handling", title: "Job Handling" },
  { id: "deployment", title: "Deployment" },
  { id: "lifecycle", title: "Worker Lifecycle" },
  { id: "ipc-protocol", title: "IPC Protocol" },
  { id: "environment", title: "Environment" },
  { id: "crash-recovery", title: "Crash Recovery" },
];

export default function WorkersPage() {
  return (
    <DocsLayout toc={tocItems}>
      <div className="flex flex-col gap-10">

          {/* Breadcrumb */}
          <div className="flex items-center gap-1.5 text-xs text-muted-foreground font-medium">
            <Link href="/" className="hover:text-foreground transition-colors">RootCX</Link>
            <ChevronRight className="h-3 w-3" />
            <Link href="/modules/data" className="hover:text-foreground transition-colors">Native Modules</Link>
            <ChevronRight className="h-3 w-3" />
            <span className="text-foreground">Workers & RPC</span>
          </div>

          {/* Title */}
          <div className="flex flex-col gap-3">
            <h1 className="text-4xl font-bold tracking-tight">Workers & RPC</h1>
            <p className="text-lg text-muted-foreground leading-7">
              Deploy TypeScript/JavaScript business logic and invoke it through a secure IPC channel.
            </p>
          </div>

          {/* Overview */}
          <section className="flex flex-col gap-4" id="overview">
            <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Overview</h2>
            <p className="text-muted-foreground leading-7">
              Workers are <strong className="text-foreground font-medium">Bun-powered Node.js-compatible processes</strong> that extend the
              Core runtime with custom business logic. While Core handles authentication, data, RBAC, and jobs
              automatically from your manifest, workers are where you write the application-specific code that Core
              cannot generate: calling third-party APIs, running AI inference, transforming data, sending emails, and
              anything else your product needs.
            </p>
            <p className="text-muted-foreground leading-7">
              Core communicates with your worker over a <strong className="text-foreground font-medium">secure IPC channel</strong> using
              JSON-line messages sent over stdin/stdout. From outside the system, clients interact through two
              surfaces:
            </p>
            <ul className="flex flex-col gap-2 text-muted-foreground leading-7 list-none pl-0">
              <li className="flex items-start gap-2">
                <span className="mt-1.5 h-1.5 w-1.5 rounded-full bg-muted-foreground/50 shrink-0" />
                <span><strong className="text-foreground font-medium">RPC</strong> — synchronous request/response calls proxied through Core to your worker. Useful for read-heavy operations or user-facing features that need a response immediately.</span>
              </li>
              <li className="flex items-start gap-2">
                <span className="mt-1.5 h-1.5 w-1.5 rounded-full bg-muted-foreground/50 shrink-0" />
                <span><strong className="text-foreground font-medium">Jobs</strong> — asynchronous tasks enqueued via the Job Queue module and dispatched to the worker's <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">handleJob</code> export. Ideal for long-running work that must not block a client request.</span>
              </li>
            </ul>
            <Callout variant="info">
              Workers run as child processes of Core, not as separate services you need to deploy independently.
              Deploying your worker is a single API call that uploads a tarball. Core manages the process lifecycle,
              environment injection, and crash recovery automatically.
            </Callout>
          </section>

          {/* Worker Structure */}
          <section className="flex flex-col gap-4" id="worker-structure">
            <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Worker Structure</h2>
            <p className="text-muted-foreground leading-7">
              A worker is a standard TypeScript or JavaScript package with an entry point at{" "}
              <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">index.ts</code> (or{" "}
              <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">index.js</code>). The entry point must export
              two named functions: <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">handleRpc</code> and{" "}
              <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">handleJob</code>. Both exports are optional —
              you can omit one if your worker only handles RPC calls or only processes jobs.
            </p>
            <CodeBlock language="text" code={`my-worker/
├── index.ts          # Entry point — exports handleRpc and/or handleJob
├── package.json      # Dependencies (bun install runs automatically on deploy)
├── tsconfig.json     # TypeScript config (optional, Bun handles TS natively)
└── lib/
    ├── email.ts      # Internal modules
    └── ai.ts`} />
            <p className="text-muted-foreground leading-7">
              The full worker entry point with both exports:
            </p>
            <CodeBlock language="typescript" code={`// index.ts — complete worker example

const RUNTIME_URL = process.env.ROOTCX_RUNTIME_URL!;
const APP_ID      = process.env.ROOTCX_APP_ID!;

// ── RPC handler ──────────────────────────────────────────────────────────────
//
// Called when a client sends POST /api/v1/apps/{appId}/rpc
// method  — the method name from the request body
// params  — the params object from the request body
// caller  — { userId, username } from the decoded JWT
//
export async function handleRpc(
  method: string,
  params: Record<string, unknown>,
  caller: { userId: string; username: string }
): Promise<unknown> {
  switch (method) {
    case "summarize": {
      const { text } = params as { text: string };
      const summary  = await runAISummarize(text);
      return { summary };
    }

    case "send-invite": {
      const { email } = params as { email: string };
      await sendInviteEmail(email, caller.username);
      return { sent: true };
    }

    default:
      throw new Error(\`Unknown RPC method: \${method}\`);
  }
}

// ── Job handler ──────────────────────────────────────────────────────────────
//
// Called when the scheduler dispatches a job from the queue
// payload — arbitrary JSON stored when the job was enqueued
//
export async function handleJob(
  payload: Record<string, unknown>
): Promise<unknown> {
  const { type } = payload;

  switch (type) {
    case "weekly-digest": {
      const { userId } = payload as { userId: string };
      const data = await fetchDigestData(userId);
      await sendDigestEmail(data);
      return { sent: true };
    }

    default:
      throw new Error(\`Unknown job type: \${type}\`);
  }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

async function runAISummarize(text: string) {
  const res = await fetch("https://api.openai.com/v1/chat/completions", {
    method:  "POST",
    headers: {
      "Content-Type":  "application/json",
      "Authorization": \`Bearer \${process.env.OPENAI_API_KEY}\`,
    },
    body: JSON.stringify({
      model:    "gpt-4o-mini",
      messages: [{ role: "user", content: \`Summarize: \${text}\` }],
    }),
  });
  const json = await res.json();
  return json.choices[0].message.content;
}

async function sendInviteEmail(email: string, fromUsername: string) {
  // Your email sending logic here
}

async function fetchDigestData(userId: string) {
  // Query the runtime API using ROOTCX_RUNTIME_URL
  const res = await fetch(
    \`\${RUNTIME_URL}/api/v1/apps/\${APP_ID}/posts?limit=10\`
  );
  return res.json();
}

async function sendDigestEmail(data: unknown) {
  // Your email sending logic here
}`} />
          </section>

          {/* Discovery */}
          <section className="flex flex-col gap-4" id="discovery">
            <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Discovery Handshake</h2>
            <p className="text-muted-foreground leading-7">
              When Core starts a worker process, the first message it sends over stdin is a{" "}
              <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">discover</code> message. The worker must
              respond with its own <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">discover</code> message
              declaring which capabilities it supports. Core waits up to 5 seconds for the discover response before
              marking the worker as failed.
            </p>
            <CodeBlock language="json" code={`// Core → Worker (sent on stdin immediately after process start)
{
  "type":       "discover",
  "appId":      "my-app",
  "runtimeUrl": "http://localhost:3000"
}

// Worker → Core (must be written to stdout within 5 seconds)
{
  "type":         "discover",
  "capabilities": ["rpc", "jobs"]
}`} />
            <p className="text-muted-foreground leading-7">
              The <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">capabilities</code> array tells Core
              which message types the worker is prepared to handle:
            </p>
            <div className="overflow-x-auto rounded-lg border border-border">
              <table className="w-full text-sm">
                <thead>
                  <tr className="border-b border-border bg-muted/30">
                    <th className="text-left px-4 py-3 font-medium text-foreground">Capability</th>
                    <th className="text-left px-4 py-3 font-medium text-foreground">Required Export</th>
                    <th className="text-left px-4 py-3 font-medium text-foreground">Description</th>
                  </tr>
                </thead>
                <tbody>
                  <tr className="border-b border-border">
                    <td className="px-4 py-3"><code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">rpc</code></td>
                    <td className="px-4 py-3"><code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">handleRpc</code></td>
                    <td className="px-4 py-3 text-muted-foreground">Worker can handle synchronous RPC method calls</td>
                  </tr>
                  <tr>
                    <td className="px-4 py-3"><code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">jobs</code></td>
                    <td className="px-4 py-3"><code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">handleJob</code></td>
                    <td className="px-4 py-3 text-muted-foreground">Worker can process asynchronous background jobs from the queue</td>
                  </tr>
                </tbody>
              </table>
            </div>
            <p className="text-muted-foreground leading-7">
              The RootCX IPC runtime handles the discover handshake automatically if you use the official SDK. When
              writing a raw worker, you must listen to{" "}
              <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">process.stdin</code> for newline-delimited
              JSON and write responses to <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">process.stdout</code>.
            </p>
          </section>

          {/* RPC Calls */}
          <section className="flex flex-col gap-4" id="rpc-calls">
            <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">RPC Calls</h2>
            <p className="text-muted-foreground leading-7">
              RPC (Remote Procedure Call) allows clients to invoke named methods on your worker synchronously through
              Core. Core acts as a proxy, forwarding the request to the worker over IPC and waiting for the response
              before replying to the HTTP client.
            </p>
            <CodeBlock language="bash" code={`POST /api/v1/apps/{appId}/rpc
Content-Type: application/json
Authorization: Bearer <token>

{
  "method": "summarize",
  "params": {
    "text": "RootCX is a backend platform that generates APIs from manifests..."
  }
}

# Success response
HTTP 200
{
  "result": {
    "summary": "RootCX generates APIs automatically from application manifests."
  }
}

# Error response (worker threw)
HTTP 500
{
  "error": "Unknown RPC method: summarize"
}`} />
            <PropertiesTable
              properties={[
                {
                  name: "method",
                  type: "string",
                  required: true,
                  description: "The method name passed as the first argument to handleRpc. Routing between methods is done inside your worker using a switch statement or similar.",
                },
                {
                  name: "params",
                  type: "object",
                  required: false,
                  description: "Arbitrary JSON object passed as the second argument to handleRpc. Defaults to an empty object.",
                },
              ]}
            />
            <h3 className="text-lg font-semibold text-foreground mt-2">Caller Context</h3>
            <p className="text-muted-foreground leading-7">
              If the request includes a valid JWT, Core decodes it and passes a{" "}
              <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">caller</code> object as the third argument
              to <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">handleRpc</code>:
            </p>
            <CodeBlock language="typescript" code={`export async function handleRpc(
  method: string,
  params: Record<string, unknown>,
  caller: {
    userId:   string;  // UUID of the authenticated user
    username: string;  // Username from the users table
  }
) {
  // Personalize the response using caller.userId
  console.log(\`RPC call from \${caller.username}: \${method}\`);
}`} />
            <h3 className="text-lg font-semibold text-foreground mt-2">Timeout</h3>
            <p className="text-muted-foreground leading-7">
              RPC calls time out after <strong className="text-foreground font-medium">30 seconds</strong>. If the worker does not respond
              within the timeout window, Core returns{" "}
              <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">HTTP 504 Gateway Timeout</code> to the
              client. Long-running operations should be offloaded to the job queue instead.
            </p>
            <CodeBlock language="bash" code={`# Call the summarize RPC method
curl -X POST https://your-runtime.com/api/v1/apps/my-app/rpc \\
  -H "Authorization: Bearer <token>" \\
  -H "Content-Type: application/json" \\
  -d '{"method": "summarize", "params": {"text": "Hello world"}}'`} />
          </section>

          {/* Job Handling */}
          <section className="flex flex-col gap-4" id="job-handling">
            <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Job Handling</h2>
            <p className="text-muted-foreground leading-7">
              When the job scheduler picks up a pending job, it sends a{" "}
              <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">job</code> IPC message to the worker. The
              worker processes it and responds with a{" "}
              <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">jobResult</code> message containing either
              a result or an error.
            </p>
            <CodeBlock language="json" code={`// Core → Worker
{
  "type":    "job",
  "id":      "3f2a1b4c-8e9d-4c2a-b1f3-7a6d5e4c3b2a",
  "payload": {
    "type":   "send-email",
    "to":     "user@example.com",
    "subject":"Welcome"
  }
}

// Worker → Core (success)
{
  "type":   "jobResult",
  "id":     "3f2a1b4c-8e9d-4c2a-b1f3-7a6d5e4c3b2a",
  "result": { "messageId": "msg_xyz789", "accepted": true },
  "error":  null
}

// Worker → Core (failure)
{
  "type":   "jobResult",
  "id":     "3f2a1b4c-8e9d-4c2a-b1f3-7a6d5e4c3b2a",
  "result": null,
  "error":  "SMTP connection refused: mail.example.com:587"
}`} />
            <p className="text-muted-foreground leading-7">
              Core maps the <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">jobResult</code> message back
              to the database row and updates the job's status, result, and error fields atomically. The{" "}
              <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">id</code> field is used to correlate
              responses when multiple jobs are in-flight concurrently.
            </p>
          </section>

          {/* Deployment */}
          <section className="flex flex-col gap-4" id="deployment">
            <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Deployment</h2>
            <p className="text-muted-foreground leading-7">
              Deploy your worker by uploading a <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">.tar.gz</code> archive
              of your worker directory to the deploy endpoint. Core extracts it, installs dependencies, and starts the
              worker automatically.
            </p>
            <CodeBlock language="bash" code={`POST /api/v1/apps/{appId}/deploy
Content-Type: application/octet-stream
Body: <binary tar.gz content>`} />
            <h3 className="text-lg font-semibold text-foreground mt-2">Deploy Steps</h3>
            <div className="flex flex-col gap-3">
              {[
                { step: "1", title: "Upload", body: "Core receives the tar.gz body and writes it to a temporary file." },
                { step: "2", title: "Extract", body: "The archive is extracted to ~/.rootcx/apps/{appId}/ , replacing any previously deployed code." },
                { step: "3", title: "Install", body: "If a package.json is present at the root of the extracted directory, Core runs bun install to install dependencies." },
                { step: "4", title: "Start", body: "Core spawns the worker process with bun run index.ts and waits for the discover handshake." },
                { step: "5", title: "Ready", body: "Once the handshake completes, Core responds HTTP 200 and the worker begins accepting RPC calls and jobs." },
              ].map(({ step, title, body }) => (
                <div key={step} className="flex gap-4 rounded-lg border border-border p-4">
                  <div className="flex h-7 w-7 shrink-0 items-center justify-center rounded-full bg-muted text-xs font-bold text-foreground">
                    {step}
                  </div>
                  <div className="flex flex-col gap-1">
                    <span className="text-sm font-semibold text-foreground">{title}</span>
                    <span className="text-sm text-muted-foreground leading-6">{body}</span>
                  </div>
                </div>
              ))}
            </div>
            <CodeBlock language="bash" code={`# Package your worker
tar -czf worker.tar.gz -C ./my-worker .

# Deploy to Core
curl -X POST https://your-runtime.com/api/v1/apps/my-app/deploy \\
  -H "Authorization: Bearer <token>" \\
  -H "Content-Type: application/octet-stream" \\
  --data-binary @worker.tar.gz

# Response
{ "ok": true, "status": "running", "pid": 12345 }`} />
            <Callout variant="info">
              Deployment replaces the running worker atomically. Core stops the old worker, extracts the new code,
              installs dependencies, and starts the new worker before responding to the deploy request. Downtime is
              typically under 2 seconds.
            </Callout>
          </section>

          {/* Worker Lifecycle */}
          <section className="flex flex-col gap-4" id="lifecycle">
            <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Worker Lifecycle</h2>
            <p className="text-muted-foreground leading-7">
              Beyond deployment, you can control the worker process directly through the lifecycle endpoints.
            </p>
            <h3 className="text-lg font-semibold text-foreground mt-2">Start Worker</h3>
            <CodeBlock language="bash" code={`POST /api/v1/apps/{appId}/worker/start

curl -X POST https://your-runtime.com/api/v1/apps/my-app/worker/start \\
  -H "Authorization: Bearer <token>"

# Response
{ "ok": true, "status": "running", "pid": 12345 }`} />
            <h3 className="text-lg font-semibold text-foreground mt-2">Stop Worker</h3>
            <CodeBlock language="bash" code={`POST /api/v1/apps/{appId}/worker/stop

curl -X POST https://your-runtime.com/api/v1/apps/my-app/worker/stop \\
  -H "Authorization: Bearer <token>"

# Response
{ "ok": true, "status": "stopped" }`} />
            <h3 className="text-lg font-semibold text-foreground mt-2">Worker Status</h3>
            <CodeBlock language="bash" code={`GET /api/v1/apps/{appId}/worker/status

curl https://your-runtime.com/api/v1/apps/my-app/worker/status \\
  -H "Authorization: Bearer <token>"

# Running worker
{
  "status":         "running",
  "pid":            12345,
  "uptime_seconds": 3600,
  "restarts":       0
}

# Stopped worker
{
  "status":         "stopped",
  "pid":            null,
  "uptime_seconds": null,
  "restarts":       2
}

# Crashed worker (exceeded crash threshold)
{
  "status":         "crashed",
  "pid":            null,
  "uptime_seconds": null,
  "restarts":       5
}`} />
          </section>

          {/* IPC Protocol */}
          <section className="flex flex-col gap-4" id="ipc-protocol">
            <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">IPC Protocol</h2>
            <p className="text-muted-foreground leading-7">
              All communication between Core and your worker happens over{" "}
              <strong className="text-foreground font-medium">newline-delimited JSON</strong> (NDJSON) on stdin/stdout. Each message is a
              single JSON object terminated by a newline character{" "}
              <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">\n</code>. Messages must not contain
              embedded newlines.
            </p>
            <h3 className="text-lg font-semibold text-foreground mt-2">Outbound (Core to Worker)</h3>
            <div className="overflow-x-auto rounded-lg border border-border">
              <table className="w-full text-sm">
                <thead>
                  <tr className="border-b border-border bg-muted/30">
                    <th className="text-left px-4 py-3 font-medium text-foreground">Type</th>
                    <th className="text-left px-4 py-3 font-medium text-foreground">Fields</th>
                    <th className="text-left px-4 py-3 font-medium text-foreground">Description</th>
                  </tr>
                </thead>
                <tbody>
                  <tr className="border-b border-border">
                    <td className="px-4 py-3"><code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">discover</code></td>
                    <td className="px-4 py-3 text-muted-foreground">appId, runtimeUrl</td>
                    <td className="px-4 py-3 text-muted-foreground">Initial handshake sent on process start</td>
                  </tr>
                  <tr className="border-b border-border">
                    <td className="px-4 py-3"><code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">rpc</code></td>
                    <td className="px-4 py-3 text-muted-foreground">id, method, params, caller</td>
                    <td className="px-4 py-3 text-muted-foreground">Proxied RPC method call from an HTTP client</td>
                  </tr>
                  <tr className="border-b border-border">
                    <td className="px-4 py-3"><code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">job</code></td>
                    <td className="px-4 py-3 text-muted-foreground">id, payload</td>
                    <td className="px-4 py-3 text-muted-foreground">Background job dispatched from the scheduler</td>
                  </tr>
                  <tr>
                    <td className="px-4 py-3"><code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">shutdown</code></td>
                    <td className="px-4 py-3 text-muted-foreground">—</td>
                    <td className="px-4 py-3 text-muted-foreground">Graceful shutdown signal. Worker should flush in-progress work and exit within 5 seconds.</td>
                  </tr>
                </tbody>
              </table>
            </div>
            <h3 className="text-lg font-semibold text-foreground mt-2">Inbound (Worker to Core)</h3>
            <div className="overflow-x-auto rounded-lg border border-border">
              <table className="w-full text-sm">
                <thead>
                  <tr className="border-b border-border bg-muted/30">
                    <th className="text-left px-4 py-3 font-medium text-foreground">Type</th>
                    <th className="text-left px-4 py-3 font-medium text-foreground">Fields</th>
                    <th className="text-left px-4 py-3 font-medium text-foreground">Description</th>
                  </tr>
                </thead>
                <tbody>
                  <tr className="border-b border-border">
                    <td className="px-4 py-3"><code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">discover</code></td>
                    <td className="px-4 py-3 text-muted-foreground">capabilities[]</td>
                    <td className="px-4 py-3 text-muted-foreground">Response to the initial handshake. Declares supported capabilities.</td>
                  </tr>
                  <tr className="border-b border-border">
                    <td className="px-4 py-3"><code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">rpcResponse</code></td>
                    <td className="px-4 py-3 text-muted-foreground">id, result, error</td>
                    <td className="px-4 py-3 text-muted-foreground">Response to an rpc message. Exactly one of result or error is non-null.</td>
                  </tr>
                  <tr className="border-b border-border">
                    <td className="px-4 py-3"><code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">jobResult</code></td>
                    <td className="px-4 py-3 text-muted-foreground">id, result, error</td>
                    <td className="px-4 py-3 text-muted-foreground">Response to a job message. Written to the jobs table by Core.</td>
                  </tr>
                  <tr>
                    <td className="px-4 py-3"><code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">log</code></td>
                    <td className="px-4 py-3 text-muted-foreground">level, message</td>
                    <td className="px-4 py-3 text-muted-foreground">Explicit log message routed to the broadcast channel. stdout/stderr are also captured automatically.</td>
                  </tr>
                </tbody>
              </table>
            </div>
          </section>

          {/* Environment */}
          <section className="flex flex-col gap-4" id="environment">
            <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Environment</h2>
            <p className="text-muted-foreground leading-7">
              Core injects environment variables into the worker process before it starts. Two variables are always set
              automatically. All other environment variables come from your application's secrets (Secrets module).
            </p>
            <div className="overflow-x-auto rounded-lg border border-border">
              <table className="w-full text-sm">
                <thead>
                  <tr className="border-b border-border bg-muted/30">
                    <th className="text-left px-4 py-3 font-medium text-foreground">Variable</th>
                    <th className="text-left px-4 py-3 font-medium text-foreground">Description</th>
                  </tr>
                </thead>
                <tbody>
                  <tr className="border-b border-border">
                    <td className="px-4 py-3"><code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">ROOTCX_APP_ID</code></td>
                    <td className="px-4 py-3 text-muted-foreground">The application ID. Use this to scope requests to the correct app when calling back to the runtime API.</td>
                  </tr>
                  <tr className="border-b border-border">
                    <td className="px-4 py-3"><code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">ROOTCX_RUNTIME_URL</code></td>
                    <td className="px-4 py-3 text-muted-foreground">The base URL of the Core runtime. Use this to make authenticated API calls back to Core from within the worker.</td>
                  </tr>
                  <tr>
                    <td className="px-4 py-3"><code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">YOUR_SECRET_NAME</code></td>
                    <td className="px-4 py-3 text-muted-foreground">All secrets stored in the Secrets module for this app are injected as environment variables with their configured names.</td>
                  </tr>
                </tbody>
              </table>
            </div>
            <CodeBlock language="typescript" code={`// Accessing environment variables in your worker
const RUNTIME_URL    = process.env.ROOTCX_RUNTIME_URL!;
const APP_ID         = process.env.ROOTCX_APP_ID!;
const OPENAI_API_KEY = process.env.OPENAI_API_KEY!;  // Set via Secrets module
const SMTP_PASSWORD  = process.env.SMTP_PASSWORD!;   // Set via Secrets module

// Call back to the runtime API
async function getEntity(entityName: string, id: string) {
  const res = await fetch(
    \`\${RUNTIME_URL}/api/v1/apps/\${APP_ID}/\${entityName}/\${id}\`
  );
  if (!res.ok) throw new Error(\`Runtime API error: \${res.status}\`);
  return res.json();
}`} />
          </section>

          {/* Crash Recovery */}
          <section className="flex flex-col gap-4" id="crash-recovery">
            <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Crash Recovery</h2>
            <p className="text-muted-foreground leading-7">
              Core monitors the worker process and automatically restarts it if it exits unexpectedly. The restart
              policy uses a crash rate limiter to prevent a broken worker from consuming all system resources in a tight
              restart loop.
            </p>
            <ul className="flex flex-col gap-2 text-muted-foreground leading-7 list-none pl-0">
              <li className="flex items-start gap-2">
                <span className="mt-1.5 h-1.5 w-1.5 rounded-full bg-muted-foreground/50 shrink-0" />
                <span>Core allows a maximum of <strong className="text-foreground font-medium">5 crashes within a 60-second window</strong>.</span>
              </li>
              <li className="flex items-start gap-2">
                <span className="mt-1.5 h-1.5 w-1.5 rounded-full bg-muted-foreground/50 shrink-0" />
                <span>Between restarts, Core applies <strong className="text-foreground font-medium">exponential backoff</strong>: 500 ms, 1 s, 2 s, 4 s, 8 s.</span>
              </li>
              <li className="flex items-start gap-2">
                <span className="mt-1.5 h-1.5 w-1.5 rounded-full bg-muted-foreground/50 shrink-0" />
                <span>If the threshold is exceeded, the worker enters the <strong className="text-foreground font-medium">Crashed</strong> state and will not be restarted automatically. You must fix the issue and redeploy or manually start the worker.</span>
              </li>
              <li className="flex items-start gap-2">
                <span className="mt-1.5 h-1.5 w-1.5 rounded-full bg-muted-foreground/50 shrink-0" />
                <span>While a worker is in the Crashed state, RPC calls return <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">503 Service Unavailable</code> and jobs accumulate in the pending queue.</span>
              </li>
            </ul>
            <Callout variant="warning">
              If your worker crashes during a job, Core resets the job from{" "}
              <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">running</code> back to{" "}
              <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">pending</code> after a 60-second timeout.
              The job will be re-dispatched once the worker recovers, incrementing the{" "}
              <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">attempts</code> counter. Ensure your job
              handlers are idempotent or guard against double-execution using the attempts counter.
            </Callout>
          </section>

        <PageNav href="/modules/workers" />
      </div>
    </DocsLayout>
  );
}
