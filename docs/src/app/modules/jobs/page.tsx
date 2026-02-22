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
  { id: "job-lifecycle", title: "Job Lifecycle" },
  { id: "enqueue-job", title: "Enqueue a Job" },
  { id: "list-jobs", title: "List Jobs" },
  { id: "get-job", title: "Get a Job" },
  { id: "worker-handler", title: "Worker Handler" },
  { id: "crash-recovery", title: "Crash Recovery" },
];

export default function JobQueuePage() {
  return (
    <DocsLayout toc={toc}>
      <div className="flex flex-col gap-10">

        {/* Breadcrumb */}
        <div className="flex items-center gap-1.5 text-xs text-muted-foreground font-medium">
          <Link href="/" className="hover:text-foreground transition-colors">RootCX</Link>
          <ChevronRight className="h-3 w-3" />
          <Link href="/modules/data" className="hover:text-foreground transition-colors">Native Modules</Link>
          <ChevronRight className="h-3 w-3" />
          <span className="text-foreground">Job Queue</span>
        </div>

        {/* Title */}
        <div className="flex flex-col gap-3">
          <h1 className="text-4xl font-bold tracking-tight">Job Queue</h1>
          <p className="text-lg text-muted-foreground leading-7">
            Durable background job processing with persistent state and at-least-once delivery, built natively on PostgreSQL.
          </p>
        </div>

        {/* Outcomes */}
        <section className="flex flex-col gap-4" id="outcomes">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Key Outcomes</h2>
          <ul className="flex flex-col gap-2 text-muted-foreground text-sm leading-7">
            {[
              "Zero extra infrastructure: No need to deploy Redis, RabbitMQ, or separate worker processes. Everything runs within the Core daemon using PostgreSQL as a reliable queue.",
              "Guaranteed execution: Jobs are persisted to the database before they are enqueued. Even if the server crashes, no jobs are lost.",
              "Seamless scheduling: Trigger background jobs instantly or schedule them for the future via a simple HTTP API or native MCP tools."
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
            The Job Queue module provides <strong className="text-foreground font-medium">durable asynchronous job processing</strong> backed
            by PostgreSQL. Jobs are persisted to the database the moment they are enqueued, guaranteeing that no work
            is lost even if Core restarts before a worker picks up the job.
          </p>
          <p className="text-muted-foreground leading-7">
            The architecture is simple: an HTTP client enqueues a job by posting a JSON payload to Core. A
            background scheduler picks up pending jobs and dispatches them to your worker process over the IPC channel.
            The worker executes the job and returns the result (or an error), which is written back to the database.
          </p>
          <p className="text-muted-foreground leading-7">
            Common use cases include:
          </p>
          <ul className="flex flex-col gap-2 text-muted-foreground leading-7 list-none pl-0">
            {[
              "Sending transactional emails after a user registration or purchase",
              "Generating reports or exports that take longer than a typical HTTP timeout",
              "Running AI agent tasks or calling external LLM APIs asynchronously",
              "Delivering webhooks to third-party services with retry semantics",
              "Scheduled data cleanup, aggregation, or sync tasks",
            ].map((item) => (
              <li key={item} className="flex items-start gap-2">
                <span className="mt-1.5 h-1.5 w-1.5 rounded-full bg-muted-foreground/50 shrink-0" />
                <span>{item}</span>
              </li>
            ))}
          </ul>
          <Callout variant="warning">
            Jobs require a <strong className="text-foreground font-medium">running worker process</strong> to be executed. If your
            worker is stopped or crashed, jobs will accumulate in the{" "}
            <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">pending</code> state and will be processed
            once the worker comes back online. No jobs are dropped.
          </Callout>
        </section>

        {/* Job Lifecycle */}
        <section className="flex flex-col gap-4" id="job-lifecycle">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Job Lifecycle</h2>
          <p className="text-muted-foreground leading-7">
            Every job moves through a well-defined set of statuses from creation to completion.
          </p>
          <div className="flex items-center gap-2 py-2 overflow-x-auto">
            {["pending", "running", "completed / failed"].map((status, i, arr) => (
              <div key={status} className="flex items-center gap-2">
                <div className="rounded-full border border-border px-4 py-1.5 text-sm font-mono text-foreground whitespace-nowrap">
                  {status}
                </div>
                {i < arr.length - 1 && (
                  <ChevronRight className="h-4 w-4 text-muted-foreground shrink-0" />
                )}
              </div>
            ))}
          </div>
          <div className="overflow-x-auto rounded-lg border border-border">
            <table className="w-full text-sm">
              <thead>
                <tr className="border-b border-border bg-muted/30">
                  <th className="text-left px-4 py-3 font-medium text-foreground">Status</th>
                  <th className="text-left px-4 py-3 font-medium text-foreground">Description</th>
                </tr>
              </thead>
              <tbody>
                <tr className="border-b border-border">
                  <td className="px-4 py-3"><code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">pending</code></td>
                  <td className="px-4 py-3 text-muted-foreground">Job has been enqueued and is waiting to be picked up by the scheduler. This is the initial state for all jobs.</td>
                </tr>
                <tr className="border-b border-border">
                  <td className="px-4 py-3"><code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">running</code></td>
                  <td className="px-4 py-3 text-muted-foreground">The scheduler has claimed the job and dispatched it to the worker. The attempts counter has been incremented.</td>
                </tr>
                <tr className="border-b border-border">
                  <td className="px-4 py-3"><code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">completed</code></td>
                  <td className="px-4 py-3 text-muted-foreground">The worker returned a result without throwing. The result is stored in the job's result field.</td>
                </tr>
                <tr>
                  <td className="px-4 py-3"><code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">failed</code></td>
                  <td className="px-4 py-3 text-muted-foreground">The worker threw an error or returned an error field. The error message is stored in the job's error field. The job is not automatically retried.</td>
                </tr>
              </tbody>
            </table>
          </div>
        </section>

        {/* Enqueue a Job */}
        <section className="flex flex-col gap-4" id="enqueue-job">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Enqueue a Job</h2>
          <p className="text-muted-foreground leading-7">
            Enqueue a new job by sending a <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">POST</code> request
            to the jobs endpoint with a JSON body. The response returns <strong className="text-foreground font-medium">201 Created</strong> with the new job ID.
          </p>
          <CodeBlock language="bash" code={`POST /api/v1/apps/{appId}/jobs
Content-Type: application/json`} />
          <PropertiesTable
            properties={[
              {
                name: "payload",
                type: "object",
                required: false,
                description: "Arbitrary JSON object passed verbatim to your worker's handleJob function. Defaults to {} if omitted.",
              },
              {
                name: "runAt",
                type: "string (ISO 8601)",
                required: false,
                description: "ISO 8601 datetime string specifying when the job should be eligible for processing. Defaults to now if omitted, creating an immediately-runnable job.",
              },
            ]}
          />
          <CodeBlock language="bash" code={`# Enqueue an immediate job
curl -X POST http://localhost:9100/api/v1/apps/my-app/jobs \\
  -H "Authorization: Bearer <token>" \\
  -H "Content-Type: application/json" \\
  -d '{
    "payload": {
      "type":  "send-email",
      "to":    "user@example.com",
      "subject": "Welcome to RootCX",
      "templateId": "welcome-v2"
    }
  }'

# 201 Created
{ "job_id": "3f2a1b4c-8e9d-4c2a-b1f3-7a6d5e4c3b2a" }

# Enqueue a delayed job (runs at a specific time)
curl -X POST http://localhost:9100/api/v1/apps/my-app/jobs \\
  -H "Authorization: Bearer <token>" \\
  -H "Content-Type: application/json" \\
  -d '{
    "payload": {
      "type":   "generate-report",
      "userId": "user_abc123",
      "format": "pdf"
    },
    "runAt": "2026-02-22T15:30:00.000Z"
  }'

# 201 Created
{ "job_id": "9c8b7a6d-5e4f-3c2b-1a0d-9e8f7a6b5c4d" }`} />
          <p className="text-muted-foreground leading-7">
            The returned <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">job_id</code> can be used to poll
            for the job's status and retrieve its result once it completes.
          </p>
        </section>

        {/* List Jobs */}
        <section className="flex flex-col gap-4" id="list-jobs">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">List Jobs</h2>
          <p className="text-muted-foreground leading-7">
            Retrieve a list of jobs for your application, optionally filtered by status. Results are ordered by{" "}
            <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">created_at</code> descending (most recent first).
          </p>
          <CodeBlock language="bash" code={`GET /api/v1/apps/{appId}/jobs?status={status}&limit={limit}`} />
          <PropertiesTable
            properties={[
              {
                name: "status",
                type: "string",
                required: false,
                description: 'Filter jobs by status. One of: "pending", "running", "completed", "failed". Omit to return all statuses.',
              },
              {
                name: "limit",
                type: "integer",
                required: false,
                description: "Maximum number of jobs to return. Defaults to 100, maximum is 1000.",
              },
            ]}
          />
          <CodeBlock language="bash" code={`# List all jobs
curl http://localhost:9100/api/v1/apps/my-app/jobs \\
  -H "Authorization: Bearer <token>"

# List only failed jobs
curl "http://localhost:9100/api/v1/apps/my-app/jobs?status=failed&limit=25" \\
  -H "Authorization: Bearer <token>"

# Response
[
  {
    "id":       "3f2a1b4c-8e9d-4c2a-b1f3-7a6d5e4c3b2a",
    "app_id":   "my-app",
    "status":   "completed",
    "payload":  { "type": "send-email", "to": "user@example.com" },
    "result":   { "messageId": "msg_xyz789", "accepted": true },
    "error":    null,
    "attempts": 1
  }
]`} />
        </section>

        {/* Get a Job */}
        <section className="flex flex-col gap-4" id="get-job">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Get a Job</h2>
          <p className="text-muted-foreground leading-7">
            Fetch a single job by its ID to inspect its current status, result, or error. Useful for polling from a
            client after enqueueing a job.
          </p>
          <CodeBlock language="bash" code={`GET /api/v1/apps/{appId}/jobs/{jobId}`} />
          <CodeBlock language="bash" code={`curl http://localhost:9100/api/v1/apps/my-app/jobs/3f2a1b4c-8e9d-4c2a-b1f3-7a6d5e4c3b2a \\
  -H "Authorization: Bearer <token>"

# Completed job
{
  "id":       "3f2a1b4c-8e9d-4c2a-b1f3-7a6d5e4c3b2a",
  "app_id":   "my-app",
  "status":   "completed",
  "payload":  { "type": "generate-report", "userId": "user_abc123" },
  "result":   { "url": "https://storage.example.com/reports/report-abc123.pdf" },
  "error":    null,
  "attempts": 1
}

# Failed job
{
  "id":       "9c8b7a6d-5e4f-3c2b-1a0d-9e8f7a6b5c4d",
  "app_id":   "my-app",
  "status":   "failed",
  "payload":  { "type": "send-email", "to": "bad@" },
  "result":   null,
  "error":    "Invalid email address: bad@",
  "attempts": 1
}`} />
        </section>

        {/* Worker Handler */}
        <section className="flex flex-col gap-4" id="worker-handler">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Worker Handler</h2>
          <p className="text-muted-foreground leading-7">
            Your worker must export a <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">handleJob</code> function.
            Core calls this function over the IPC channel whenever a job is ready to be processed. The
            function receives the job payload and must return a result object or throw an error.
          </p>
          <CodeBlock language="typescript" code={`// worker/index.ts

export async function handleJob(payload: Record<string, unknown>) {
  const { type } = payload;

  switch (type) {
    case "send-email": {
      const { to, subject, templateId } = payload as {
        to: string;
        subject: string;
        templateId: string;
      };

      // Call your email provider SDK
      const result = await sendEmail({ to, subject, templateId });

      // Whatever you return here is stored in the job's result field
      return { messageId: result.id, accepted: true };
    }

    case "generate-report": {
      const { userId, format } = payload as {
        userId: string;
        format: "pdf" | "csv";
      };

      const url = await buildReport(userId, format);

      return { url };
    }

    default:
      throw new Error(\`Unknown job type: \${type}\`);
  }
}`} />
          <p className="text-muted-foreground leading-7">
            The return value of <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">handleJob</code> must be
            JSON-serializable. It is stored verbatim in the job's{" "}
            <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">result</code> field and can be
            retrieved via the Get Job endpoint. If the function throws, the error message is stored in the{" "}
            <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">error</code> field and the job
            transitions to <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">failed</code>.
          </p>
        </section>

        {/* Crash Recovery */}
        <section className="flex flex-col gap-4" id="crash-recovery">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Crash Recovery</h2>
          <p className="text-muted-foreground leading-7">
            The job queue provides <strong className="text-foreground font-medium">at-least-once delivery</strong> semantics. If the worker
            crashes while a job is in the <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">running</code> state,
            the scheduler detects the stale job and resets it back
            to <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">pending</code> so it can be re-dispatched
            once the worker recovers. This means a job may be delivered more than once in the event of a crash, so
            your <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">handleJob</code> function should be
            designed to be idempotent where possible.
          </p>
          <Callout variant="info">
            Because stale running jobs are reset to pending, a job that crashes the worker repeatedly can be attempted
            multiple times. Use the <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">attempts</code> counter
            inside your handler to implement a max-attempts guard if needed.
          </Callout>
        </section>

        <PageNav href="/modules/jobs" />
      </div>
    </DocsLayout>
  );
}
