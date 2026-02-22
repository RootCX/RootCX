import { DocsLayout } from "@/components/layout/DocsLayout";
import { PageNav } from "@/components/docs/PageNav";
import { CodeBlock } from "@/components/docs/CodeBlock";
import { Callout } from "@/components/docs/Callout";
import { PropertiesTable } from "@/components/docs/PropertiesTable";
import { ChevronRight } from "lucide-react";
import Link from "next/link";

const toc = [
  { id: "outcomes", title: "Key Outcomes" },
  { id: "overview", title: "Overview" },
  { id: "backend-structure", title: "Backend Structure" },
  { id: "rpc-calls", title: "RPC Calls" },
  { id: "deployment", title: "Deployment" },
  { id: "lifecycle", title: "Lifecycle" },
  { id: "environment", title: "Environment" },
  { id: "crash-recovery", title: "Crash Recovery" },
];

export default function BackendPage() {
  return (
    <DocsLayout toc={toc}>
      <div className="flex flex-col gap-10">

        {/* Breadcrumb */}
        <div className="flex items-center gap-1.5 text-xs text-muted-foreground font-medium">
          <Link href="/" className="hover:text-foreground transition-colors">RootCX</Link>
          <ChevronRight className="h-3 w-3" />
          <Link href="/modules/data" className="hover:text-foreground transition-colors">Native Modules</Link>
          <ChevronRight className="h-3 w-3" />
          <span className="text-foreground">Backend & RPC</span>
        </div>

        {/* Title */}
        <div className="flex flex-col gap-3">
          <h1 className="text-4xl font-bold tracking-tight">Backend & RPC</h1>
          <p className="text-lg text-muted-foreground leading-7">
            Deploy custom TypeScript/JavaScript business logic as managed processes and invoke it through RPC calls proxied by the Core daemon.
          </p>
        </div>

        {/* Outcomes */}
        <section className="flex flex-col gap-4" id="outcomes">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Key Outcomes</h2>
          <ul className="flex flex-col gap-2 text-muted-foreground text-sm leading-7">
            {[
              "Zero infrastructure: Deploy standalone code without configuring servers, managing ports, or setting up containers. Core handles execution inside an isolated child process.",
              "Direct Core integration: Backends communicate over stdin/stdout IPC with no network overhead, acting as native extensions of the Core daemon.",
              "Universal execution: Run AI inference, schedule background jobs, handle webhooks, and process complex transactions safely on the server side.",
            ].map((item, i) => (
              <li key={i} className="flex items-start gap-2">
                <span className="mt-2 flex-shrink-0 w-1.5 h-1.5 rounded-full bg-primary/60" />
                <span dangerouslySetInnerHTML={{ __html: item.replace(/^([^:]+:)/, '<strong>$1</strong>') }} />
              </li>
            ))}
          </ul>
        </section>

        {/* Overview */}
        <section className="flex flex-col gap-4" id="overview">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Overview</h2>
          <p className="text-muted-foreground leading-7">
            Applications need more than a UI connected to a database. Payment processing, email delivery, third-party API integration, and AI inference all require server-side logic that cannot run in a browser. A <strong>Backend</strong> is where that logic lives. You write standard TypeScript or JavaScript, package it into a tarball, and upload it to Core with a single API call.
          </p>
          <p className="text-muted-foreground leading-7">
            Core spawns your backend as an <strong>isolated child process</strong> running under Bun. All communication between Core and the process happens over a <strong className="text-foreground font-medium">JSON-line IPC channel</strong> on stdin/stdout. On startup, Core sends a discovery message and the process responds with the capabilities it supports (RPC, jobs, or both). Once the handshake completes, the backend is ready to receive work.
          </p>
          <p className="text-muted-foreground leading-7">
            From outside the system, clients interact through two surfaces: <strong className="text-foreground font-medium">RPC</strong> for synchronous request/response calls proxied through Core, and <strong className="text-foreground font-medium">Jobs</strong> for asynchronous tasks dispatched from the job queue. Core manages the full process lifecycle, including environment injection, crash recovery, and automatic restarts.
          </p>
          <Callout variant="info">
            Backends run as child processes of Core, not as separate services. Deploying is a single API call
            that uploads a tarball. Core handles dependency installation, process management, and crash recovery automatically.
          </Callout>
        </section>

        {/* Backend Structure */}
        <section className="flex flex-col gap-4" id="backend-structure">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Backend Structure</h2>
          <p className="text-muted-foreground leading-7">
            A backend is a standard TypeScript or JavaScript package with an entry point at{" "}
            <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">index.ts</code> (or{" "}
            <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">index.js</code>). The entry point must export
            two named functions: <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">handleRpc</code> and{" "}
            <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">handleJob</code>. Both exports are optional —
            you can omit one if your backend only handles RPC calls or only processes jobs.
          </p>
          <CodeBlock language="text" code={`my-backend/
├── index.ts          # Entry point — exports handleRpc and/or handleJob
├── package.json      # Dependencies (bun install runs automatically on deploy)
├── tsconfig.json     # TypeScript config (optional, Bun handles TS natively)
└── lib/
    ├── email.ts      # Internal modules
    └── ai.ts`} />
          <p className="text-muted-foreground leading-7">
            The full backend entry point with both exports:
          </p>
          <CodeBlock language="typescript" code={`// index.ts — complete backend example

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
  // Query the Core API using ROOTCX_RUNTIME_URL
  const res = await fetch(
    \`\${RUNTIME_URL}/api/v1/apps/\${APP_ID}/posts?limit=10\`
  );
  return res.json();
}

async function sendDigestEmail(data: unknown) {
  // Your email sending logic here
}`} />
        </section>

        {/* RPC Calls */}
        <section className="flex flex-col gap-4" id="rpc-calls">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">RPC Calls</h2>
          <p className="text-muted-foreground leading-7">
            RPC (Remote Procedure Call) allows clients to invoke named methods on your backend synchronously through
            Core. Core proxies the request over IPC to the backend process and waits for the response before replying
            to the HTTP client. RPC calls time out after <strong className="text-foreground font-medium">30 seconds</strong> — long-running
            operations should be offloaded to the job queue instead.
          </p>
          <CodeBlock language="bash" code={`POST /api/v1/apps/{appId}/rpc
Content-Type: application/json
Authorization: Bearer <token>`} />
          <PropertiesTable
            properties={[
              {
                name: "method",
                type: "string",
                required: true,
                description: "The method name passed as the first argument to handleRpc. Routing between methods is done inside your backend using a switch statement or similar.",
              },
              {
                name: "params",
                type: "object",
                required: false,
                description: "Arbitrary JSON object passed as the second argument to handleRpc. Defaults to an empty object.",
              },
              {
                name: "id",
                type: "string",
                required: false,
                description: "Optional correlation ID. Echoed back in the response for client-side request tracking.",
              },
            ]}
          />
          <CodeBlock language="bash" code={`# Call the summarize RPC method
curl -X POST http://localhost:9100/api/v1/apps/my-app/rpc \\
  -H "Authorization: Bearer <token>" \\
  -H "Content-Type: application/json" \\
  -d '{"method": "summarize", "params": {"text": "Hello world"}}'

# Response (HTTP 200)
{
  "id": "...",
  "result": {
    "summary": "RootCX generates APIs automatically from application manifests."
  },
  "error": null
}

# Error response (worker threw)
{
  "id": "...",
  "result": null,
  "error": "Unknown RPC method: summarize"
}`} />
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
  console.log(\`RPC call from \${caller.username}: \${method}\`);
}`} />
        </section>

        {/* Deployment */}
        <section className="flex flex-col gap-4" id="deployment">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Deployment</h2>
          <p className="text-muted-foreground leading-7">
            Deploy your backend by uploading a <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">.tar.gz</code> archive
            to the deploy endpoint as <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">multipart/form-data</code> with
            the field name <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">archive</code>. The maximum upload size
            is <strong className="text-foreground font-medium">50 MB</strong>. Core extracts the archive, runs{" "}
            <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">bun install</code> if a{" "}
            <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">package.json</code> is present, and starts the
            backend process automatically.
          </p>
          <CodeBlock language="bash" code={`POST /api/v1/apps/{appId}/deploy
Content-Type: multipart/form-data`} />
          <CodeBlock language="bash" code={`# Package your backend
tar -czf worker.tar.gz -C ./my-backend .

# Deploy to Core
curl -X POST http://localhost:9100/api/v1/apps/my-app/deploy \\
  -H "Authorization: Bearer <token>" \\
  -F "archive=@worker.tar.gz"

# Response (HTTP 200)
{ "message": "app 'my-app' deployed and started" }`} />
          <Callout variant="info">
            Deployment replaces any previously deployed code atomically. Core stops the old process, extracts the new
            archive, installs dependencies, and starts the new process before responding.
          </Callout>
        </section>

        {/* Lifecycle */}
        <section className="flex flex-col gap-4" id="lifecycle">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Lifecycle</h2>
          <p className="text-muted-foreground leading-7">
            Beyond deployment, you can control the backend process directly through lifecycle endpoints.
          </p>
          <h3 className="text-lg font-semibold text-foreground mt-2">Start</h3>
          <CodeBlock language="bash" code={`curl -X POST http://localhost:9100/api/v1/apps/my-app/worker/start \\
  -H "Authorization: Bearer <token>"

# Response (HTTP 200)
{ "message": "worker 'my-app' started" }`} />
          <h3 className="text-lg font-semibold text-foreground mt-2">Stop</h3>
          <CodeBlock language="bash" code={`curl -X POST http://localhost:9100/api/v1/apps/my-app/worker/stop \\
  -H "Authorization: Bearer <token>"

# Response (HTTP 200)
{ "message": "worker 'my-app' stopped" }`} />
          <h3 className="text-lg font-semibold text-foreground mt-2">Status</h3>
          <CodeBlock language="bash" code={`curl http://localhost:9100/api/v1/apps/my-app/worker/status \\
  -H "Authorization: Bearer <token>"

# Response (HTTP 200)
{ "app_id": "my-app", "status": "running" }`} />
        </section>

        {/* Environment */}
        <section className="flex flex-col gap-4" id="environment">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Environment</h2>
          <p className="text-muted-foreground leading-7">
            Core injects environment variables into the backend process before it starts. Two variables are always
            present. All additional variables come from your application's secrets configured through the Secrets module.
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
                  <td className="px-4 py-3 text-muted-foreground">The application ID. Use this to scope API requests to the correct app.</td>
                </tr>
                <tr className="border-b border-border">
                  <td className="px-4 py-3"><code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">ROOTCX_RUNTIME_URL</code></td>
                  <td className="px-4 py-3 text-muted-foreground">The base URL of the Core API. Use this to make API calls back to Core from within the backend.</td>
                </tr>
                <tr>
                  <td className="px-4 py-3"><code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">YOUR_SECRET_NAME</code></td>
                  <td className="px-4 py-3 text-muted-foreground">All secrets stored in the Secrets module for this app are injected as environment variables with their configured names.</td>
                </tr>
              </tbody>
            </table>
          </div>
          <CodeBlock language="typescript" code={`// Accessing environment variables in your backend
const RUNTIME_URL    = process.env.ROOTCX_RUNTIME_URL!;
const APP_ID         = process.env.ROOTCX_APP_ID!;
const OPENAI_API_KEY = process.env.OPENAI_API_KEY!;  // Set via Secrets module

// Call back to the Core API
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
            Core monitors the backend process and automatically restarts it if it exits unexpectedly. The restart
            policy uses a crash rate limiter to prevent a broken backend from consuming system resources in a tight
            restart loop.
          </p>
          <ul className="flex flex-col gap-2 text-muted-foreground leading-7 list-none pl-0">
            <li className="flex items-start gap-2">
              <span className="mt-1.5 h-1.5 w-1.5 rounded-full bg-muted-foreground/50 shrink-0" />
              <span>Core allows a maximum of <strong className="text-foreground font-medium">5 crashes within a 60-second window</strong>.</span>
            </li>
            <li className="flex items-start gap-2">
              <span className="mt-1.5 h-1.5 w-1.5 rounded-full bg-muted-foreground/50 shrink-0" />
              <span>Between restarts, Core applies <strong className="text-foreground font-medium">exponential backoff</strong> with a 2-second base delay (2s, 4s, 8s, 16s, ...).</span>
            </li>
            <li className="flex items-start gap-2">
              <span className="mt-1.5 h-1.5 w-1.5 rounded-full bg-muted-foreground/50 shrink-0" />
              <span>If the crash threshold is exceeded, the backend enters the <strong className="text-foreground font-medium">crashed</strong> state and stops restarting automatically. You must fix the issue and redeploy or manually start the process.</span>
            </li>
            <li className="flex items-start gap-2">
              <span className="mt-1.5 h-1.5 w-1.5 rounded-full bg-muted-foreground/50 shrink-0" />
              <span>While in the crashed state, RPC calls return <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">503 Service Unavailable</code> and jobs accumulate in the pending queue.</span>
            </li>
          </ul>
          <Callout variant="warning">
            If your backend crashes during a job, Core resets the job from{" "}
            <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">running</code> back to{" "}
            <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">pending</code> after a 60-second timeout.
            The job will be re-dispatched once the backend recovers. Ensure your job handlers are idempotent.
          </Callout>
        </section>

        <PageNav href="/modules/backend" />
      </div>
    </DocsLayout>
  );
}
