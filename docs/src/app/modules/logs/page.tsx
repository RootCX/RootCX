import { DocsLayout } from "@/components/layout/DocsLayout";
import { PageNav } from "@/components/docs/PageNav";
import { CodeBlock } from "@/components/docs/CodeBlock";
import { Callout } from "@/components/docs/Callout";
import { ChevronRight } from "lucide-react";
import Link from "next/link";

const toc = [
  { id: "outcomes", title: "Key Outcomes" },
  { id: "overview", title: "Overview" },
  { id: "sse-endpoint", title: "SSE Endpoint" },
  { id: "log-format", title: "Log Format" },
  { id: "backend-logging", title: "Backend Logging" },
  { id: "client-example", title: "Client Example" },
];

export default function LogsPage() {
  return (
    <DocsLayout toc={toc}>
      <div className="flex flex-col gap-10">

        <div className="flex items-center gap-1.5 text-xs text-muted-foreground font-medium">
          <Link href="/" className="hover:text-foreground transition-colors">RootCX</Link>
          <ChevronRight className="h-3 w-3" />
          <Link href="/modules/data" className="hover:text-foreground transition-colors">Native Modules</Link>
          <ChevronRight className="h-3 w-3" />
          <span className="text-foreground">Real-time Logs</span>
        </div>

        <div className="flex flex-col gap-3">
          <h1 className="text-4xl font-bold tracking-tight">Real-time Logs</h1>
          <p className="text-lg text-muted-foreground leading-7">
            Stream live output from your Backend process via Server-Sent Events with zero configuration.
          </p>
        </div>

        <section className="flex flex-col gap-4" id="outcomes">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Key Outcomes</h2>
          <ul className="flex flex-col gap-2 text-muted-foreground text-sm leading-7">
            {[
              "Zero-config streaming: No need to set up Kafka or Redis streams. Core automatically broadcasts all Backend output over an in-memory channel.",
              "Frictionless debugging: Catch bugs in real-time by streaming live logs into the built-in Studio Console alongside your code editor.",
              "Native observability: Subscribe observability tools to the SSE endpoint to forward logs without overhead or SDKs."
            ].map((item, i) => (
              <li key={i} className="flex items-start gap-2">
                <span className="mt-2 flex-shrink-0 w-1.5 h-1.5 rounded-full bg-primary/60" />
                <span dangerouslySetInnerHTML={{ __html: item.replace(/^([^:]+:)/, '<strong>$1</strong>') }} />
              </li>
            ))}
          </ul>
        </section>

        <section className="flex flex-col gap-4" id="overview">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Overview</h2>
          <p className="text-muted-foreground leading-7">
            Every time your Backend writes to stdout or stderr — via{" "}
            <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">console.log</code>,{" "}
            <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">console.warn</code>, or{" "}
            <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">console.error</code> — Core
            captures the output and forwards it to an in-memory broadcast channel. Any number of HTTP clients
            can subscribe to this channel via <strong className="text-foreground font-medium">Server-Sent Events</strong> (SSE).
          </p>
          <p className="text-muted-foreground leading-7">
            This is the same mechanism used by the Studio Console panel. You can also subscribe from any HTTP
            client, build your own log viewer, or forward logs to external observability tools.
          </p>
          <Callout variant="info" title="Ephemeral logs">
            Logs are held in an in-memory broadcast channel and are <strong className="text-foreground font-medium">not persisted</strong> to
            the database. They exist only while Core is running. For durable log storage, write structured records
            to your app{"'"}s entity tables from within your Backend.
          </Callout>
        </section>

        <section className="flex flex-col gap-4" id="sse-endpoint">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">SSE Endpoint</h2>
          <p className="text-muted-foreground leading-7">
            Open a long-lived HTTP connection to the logs endpoint. The server sends{" "}
            <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">text/event-stream</code> data
            as each log line arrives.
          </p>

          <div className="rounded-lg border border-border bg-card p-4 flex flex-col gap-2">
            <div className="flex items-center gap-3">
              <span className="text-xs font-mono font-bold text-emerald-400 bg-emerald-400/10 rounded px-2 py-0.5">GET</span>
              <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">
                /api/v1/apps/{"{appId}"}/logs
              </code>
            </div>
          </div>

          <h3 className="text-lg font-semibold text-foreground mt-2">curl Example</h3>
          <CodeBlock language="bash" code={`curl -N http://localhost:9100/api/v1/apps/my-app/logs \\
  -H "Authorization: Bearer <token>"`} />
          <p className="text-muted-foreground leading-7">
            The <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">-N</code> flag
            disables buffering so events display as they arrive. The connection remains open until you close it or
            the Backend stops.
          </p>

          <h3 className="text-lg font-semibold text-foreground mt-2">Event Format</h3>
          <CodeBlock language="text" code={`data: {"level":"info","message":"Worker started"}

data: {"level":"stdout","message":"Processing order ORD-123"}

data: {"level":"system","message":"worker restarted (attempt 2)"}

data: {"level":"error","message":"Failed to send email: SMTP timeout"}`} />
        </section>

        <section className="flex flex-col gap-4" id="log-format">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Log Format</h2>
          <p className="text-muted-foreground leading-7">
            Every log event is a JSON object with two fields:
          </p>
          <div className="rounded-lg border border-border overflow-hidden">
            <table className="w-full text-sm">
              <thead>
                <tr className="border-b border-border bg-muted/30">
                  <th className="px-4 py-3 text-left font-medium text-foreground">Field</th>
                  <th className="px-4 py-3 text-left font-medium text-foreground">Type</th>
                  <th className="px-4 py-3 text-left font-medium text-foreground">Description</th>
                </tr>
              </thead>
              <tbody>
                <tr className="border-b border-border">
                  <td className="px-4 py-3 font-mono text-xs text-foreground">level</td>
                  <td className="px-4 py-3 text-xs text-muted-foreground font-mono">string</td>
                  <td className="px-4 py-3 text-xs text-muted-foreground leading-relaxed">Log severity level</td>
                </tr>
                <tr>
                  <td className="px-4 py-3 font-mono text-xs text-foreground">message</td>
                  <td className="px-4 py-3 text-xs text-muted-foreground font-mono">string</td>
                  <td className="px-4 py-3 text-xs text-muted-foreground leading-relaxed">The log message content</td>
                </tr>
              </tbody>
            </table>
          </div>

          <h3 className="text-lg font-semibold text-foreground mt-2">Log Levels</h3>
          <div className="rounded-lg border border-border overflow-hidden">
            <table className="w-full text-sm">
              <thead>
                <tr className="border-b border-border bg-muted/30">
                  <th className="px-4 py-3 text-left font-medium text-foreground">Level</th>
                  <th className="px-4 py-3 text-left font-medium text-foreground">Source</th>
                </tr>
              </thead>
              <tbody>
                {[
                  ["stdout", "Non-JSON stdout output from the Backend process"],
                  ["stderr", "Stderr output from the Backend process"],
                  ["info", "Explicit IPC log messages (default level)"],
                  ["warn", "Explicit IPC log messages with warn level"],
                  ["error", "Explicit IPC log messages with error level"],
                  ["debug", "Explicit IPC log messages with debug level"],
                  ["system", "Supervisor lifecycle events (start, stop, crash, restart)"],
                ].map(([level, source], i) => (
                  <tr key={i} className="border-b border-border last:border-0">
                    <td className="px-4 py-3 font-mono text-xs text-foreground">{level}</td>
                    <td className="px-4 py-3 text-xs text-muted-foreground">{source}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </section>

        <section className="flex flex-col gap-4" id="backend-logging">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Backend Logging</h2>
          <p className="text-muted-foreground leading-7">
            Backends emit logs by writing to stdout or stderr. Standard{" "}
            <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">console.*</code> calls
            work out of the box — no SDK required.
          </p>
          <CodeBlock language="typescript" code={`export async function handleRpc(method: string, params: Record<string, unknown>) {
  console.log("RPC called:", method);        // level: stdout
  console.error("Something went wrong");     // level: stderr

  // Or send explicit IPC log messages for precise level control:
  process.stdout.write(JSON.stringify({
    type: "log",
    level: "info",
    message: "Structured log entry"
  }) + "\\n");

  return { ok: true };
}`} />
          <Callout variant="info" title="No log replay">
            Clients only receive logs emitted after they connect. There is no history buffer or persistence.
            If you need to persist logs, write them to an entity table from your Backend.
          </Callout>
        </section>

        <section className="flex flex-col gap-4" id="client-example">
          <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Client Example</h2>
          <p className="text-muted-foreground leading-7">
            Subscribe to the log stream using the{" "}
            <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">fetch</code> API
            for authenticated connections:
          </p>
          <CodeBlock language="typescript" code={`const response = await fetch(
  "http://localhost:9100/api/v1/apps/my-app/logs",
  {
    headers: { Authorization: \`Bearer \${accessToken}\` },
  }
);

const reader = response.body!.getReader();
const decoder = new TextDecoder();

while (true) {
  const { done, value } = await reader.read();
  if (done) break;

  const lines = decoder.decode(value).split("\\n");
  for (const line of lines) {
    if (line.startsWith("data: ")) {
      const log = JSON.parse(line.slice(6));
      console.log(\`[\${log.level}] \${log.message}\`);
    }
  }
}`} />
        </section>

        <PageNav href="/modules/logs" />
      </div>
    </DocsLayout>
  );
}
