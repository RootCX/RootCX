import { DocsLayout } from "@/components/layout/DocsLayout";
import { PageNav } from "@/components/docs/PageNav";
import { CodeBlock } from "@/components/docs/CodeBlock";
import { Callout } from "@/components/docs/Callout";
import { ChevronRight } from "lucide-react";
import Link from "next/link";

const toc = [
    { id: "overview", title: "Overview" },
    { id: "sse-endpoint", title: "SSE endpoint" },
    { id: "log-format", title: "Log format" },
    { id: "worker-logging", title: "Worker logging" },
    { id: "multiple-clients", title: "Multiple clients" },
    { id: "studio-console", title: "Studio Console" },
    { id: "client-example", title: "Client example" },
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

                <header className="flex flex-col gap-4">
                    <h1 className="text-4xl font-semibold tracking-tight lg:text-5xl">Real-time Logs</h1>
                    <p className="text-lg text-muted-foreground max-w-2xl leading-relaxed">
                        Stream live stdout and stderr output from your workers using Server-Sent Events — no polling required.
                    </p>
                </header>

                <section className="flex flex-col gap-4" id="overview">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Overview</h2>
                    <p className="text-muted-foreground leading-7">
                        Every time your worker calls <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">console.log</code>, <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">console.warn</code>, or <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">console.error</code>, the Core daemon captures it and forwards it to an in-memory broadcast channel. Any number of HTTP clients can subscribe to this channel and receive log messages in real time via <strong className="text-foreground font-medium">Server-Sent Events</strong> (SSE).
                    </p>
                    <p className="text-muted-foreground leading-7">
                        This is the same mechanism used by the Studio Console panel. You can also subscribe from any HTTP client, build your own log viewer, or forward logs to external observability tools.
                    </p>
                    <Callout variant="info" title="Ephemeral logs">
                        Logs are held in an in-memory broadcast channel and are <strong className="text-foreground font-medium">not persisted</strong> to the database. They exist only while the daemon is running and the channel has subscribers. For durable log storage, write structured records to your app's entity tables from within your worker.
                    </Callout>
                </section>

                <section className="flex flex-col gap-4" id="sse-endpoint">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">SSE endpoint</h2>
                    <p className="text-muted-foreground leading-7">
                        Open a long-lived HTTP connection to the logs endpoint. The server sends <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">text/event-stream</code> data as each log line arrives.
                    </p>
                    <CodeBlock language="text" code={`GET /api/v1/apps/{appId}/logs

Response headers:
  Content-Type: text/event-stream
  Cache-Control: no-cache
  Connection: keep-alive
  X-Accel-Buffering: no`} />
                    <p className="text-muted-foreground leading-7">
                        Each event is a JSON object on a single line:
                    </p>
                    <CodeBlock language="text" code={`data: {"level":"info","message":"Worker started","timestamp":"2024-01-15T10:30:00.123Z"}

data: {"level":"info","message":"Processing job: { id: 'job-abc' }","timestamp":"2024-01-15T10:30:01.456Z"}

data: {"level":"warn","message":"Retrying API call (attempt 2/3)","timestamp":"2024-01-15T10:30:02.789Z"}

data: {"level":"error","message":"Failed to send email: SMTP timeout","timestamp":"2024-01-15T10:30:03.012Z"}`} />
                    <p className="text-muted-foreground leading-7">
                        Subscribe via curl to see live logs in your terminal:
                    </p>
                    <CodeBlock language="bash" code={`curl -N http://localhost:9100/api/v1/apps/crm/logs \\
  -H 'Authorization: Bearer <token>'`} />
                    <p className="text-muted-foreground leading-7">
                        The connection remains open until you close it or the worker stops. The <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">-N</code> flag (no buffering) is important for curl to display SSE events as they arrive.
                    </p>
                </section>

                <section className="flex flex-col gap-4" id="log-format">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Log format</h2>
                    <p className="text-muted-foreground leading-7">
                        Every log event has three fields:
                    </p>
                    <div className="rounded-lg border border-border overflow-hidden">
                        <table className="w-full text-sm">
                            <thead>
                                <tr className="border-b border-border bg-[#0d0d0d]">
                                    <th className="px-4 py-3 text-left font-semibold">Field</th>
                                    <th className="px-4 py-3 text-left font-semibold">Type</th>
                                    <th className="px-4 py-3 text-left font-semibold">Values</th>
                                    <th className="px-4 py-3 text-left font-semibold">Description</th>
                                </tr>
                            </thead>
                            <tbody>
                                {[
                                    ["level", "string", "info, warn, error", "Log severity. Maps to console.log → info, console.warn → warn, console.error → error."],
                                    ["message", "string", "—", "The formatted string passed to console.*. Multi-argument calls are space-joined."],
                                    ["timestamp", "string (ISO 8601)", "—", "UTC timestamp of when the log was captured by the Core daemon."],
                                ].map(([field, type, values, desc], i) => (
                                    <tr key={i} className="border-b border-border/50 last:border-0">
                                        <td className="px-4 py-3 font-mono text-xs text-primary">{field}</td>
                                        <td className="px-4 py-3 text-xs text-muted-foreground font-mono">{type}</td>
                                        <td className="px-4 py-3 text-xs text-muted-foreground font-mono">{values}</td>
                                        <td className="px-4 py-3 text-xs text-muted-foreground leading-relaxed">{desc}</td>
                                    </tr>
                                ))}
                            </tbody>
                        </table>
                    </div>
                </section>

                <section className="flex flex-col gap-4" id="worker-logging">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Worker logging</h2>
                    <p className="text-muted-foreground leading-7">
                        Workers emit logs by writing to stdout or stderr. No special SDK is required — standard <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">console.*</code> calls work out of the box.
                    </p>
                    <CodeBlock language="typescript" filename="worker/index.ts" code={`export async function handleRpc(
  method: string,
  params: Record<string, unknown>,
  caller?: { userId: string; username: string }
) {
  console.log("RPC called:", method, "by user:", caller?.username);

  if (method === "processOrder") {
    const { orderId } = params as { orderId: string };
    console.log("Processing order", orderId);

    try {
      const result = await processOrder(orderId);
      console.log("Order processed successfully:", orderId);
      return result;
    } catch (err) {
      console.error("Failed to process order", orderId, err);
      throw err;
    }
  }

  throw new Error(\`Unknown method: \${method}\`);
}

export async function handleJob(payload: Record<string, unknown>) {
  console.log("Job received:", JSON.stringify(payload));
  // ... processing ...
  console.warn("Job completed with warnings");
  return { processed: true };
}`} />
                    <p className="text-muted-foreground leading-7">
                        Workers can also send explicit log messages via the IPC protocol. This is useful for structured log entries where you need precise control over the level and message:
                    </p>
                    <CodeBlock language="typescript" code={`// Explicit IPC log message
process.stdout.write(JSON.stringify({
  type: "log",
  level: "info",
  message: "Structured log entry with context"
}) + "\\n");`} />
                </section>

                <section className="flex flex-col gap-4" id="multiple-clients">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Multiple clients</h2>
                    <p className="text-muted-foreground leading-7">
                        The broadcast channel supports multiple simultaneous subscribers. Each client that connects to the SSE endpoint receives the same stream of events from that point forward.
                    </p>
                    <div className="rounded-lg border border-border overflow-hidden">
                        <table className="w-full text-sm">
                            <thead>
                                <tr className="border-b border-border bg-[#0d0d0d]">
                                    <th className="px-4 py-3 text-left font-semibold">Property</th>
                                    <th className="px-4 py-3 text-left font-semibold">Value</th>
                                </tr>
                            </thead>
                            <tbody>
                                {[
                                    ["Channel capacity", "256 messages"],
                                    ["Overflow behavior", "Old messages dropped (newest wins)"],
                                    ["Reconnect behavior", "Client re-subscribes, no replay of missed messages"],
                                    ["Max concurrent subscribers", "Unlimited (one goroutine per subscriber)"],
                                ].map(([prop, val], i) => (
                                    <tr key={i} className="border-b border-border/50 last:border-0">
                                        <td className="px-4 py-3 text-sm text-foreground">{prop}</td>
                                        <td className="px-4 py-3 text-sm text-muted-foreground font-mono">{val}</td>
                                    </tr>
                                ))}
                            </tbody>
                        </table>
                    </div>
                    <Callout variant="warning" title="No log replay">
                        Clients only receive logs emitted after they connect. There is no history buffer. If your use case requires persisting logs, write them to an entity table from your worker and query them via the CRUD API.
                    </Callout>
                </section>

                <section className="flex flex-col gap-4" id="studio-console">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Studio Console</h2>
                    <p className="text-muted-foreground leading-7">
                        The Console panel in RootCX Studio is a built-in log viewer that subscribes to this SSE endpoint automatically when a worker is running. It displays logs with color-coded severity levels, timestamps, and a clear button.
                    </p>
                    <div className="grid grid-cols-1 sm:grid-cols-3 gap-3">
                        {[
                            { label: "info", color: "text-blue-400", bg: "bg-blue-500/5 border-blue-500/20", desc: "Standard output from console.log" },
                            { label: "warn", color: "text-yellow-400", bg: "bg-yellow-500/5 border-yellow-500/20", desc: "Warnings from console.warn" },
                            { label: "error", color: "text-red-400", bg: "bg-red-500/5 border-red-500/20", desc: "Errors from console.error or uncaught exceptions" },
                        ].map((level, i) => (
                            <div key={i} className={`rounded-lg border p-4 ${level.bg}`}>
                                <span className={`font-mono text-xs font-bold ${level.color}`}>{level.label}</span>
                                <p className="text-xs text-muted-foreground mt-1 leading-relaxed">{level.desc}</p>
                            </div>
                        ))}
                    </div>
                    <p className="text-muted-foreground leading-7">
                        The Studio Console reconnects automatically if the worker restarts or if the SSE connection drops. You can also open the Console by pressing <kbd className="rounded border border-border bg-[#1e1e1e] px-1.5 py-0.5 font-mono text-xs">⌘`</kbd> from anywhere in Studio.
                    </p>
                </section>

                <section className="flex flex-col gap-4" id="client-example">
                    <h2 className="text-2xl font-semibold tracking-tight border-b border-border pb-3">Client example</h2>
                    <p className="text-muted-foreground leading-7">
                        Subscribe to the log stream from a browser or Node.js using the <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">EventSource</code> API:
                    </p>
                    <CodeBlock language="typescript" filename="log-viewer.ts" code={`const es = new EventSource(
  "http://localhost:9100/api/v1/apps/crm/logs",
  {
    // EventSource doesn't support custom headers natively.
    // Use a proxy or pass the token as a query param if needed.
  }
);

es.onmessage = (event) => {
  const log = JSON.parse(event.data) as {
    level: "info" | "warn" | "error";
    message: string;
    timestamp: string;
  };

  const time = new Date(log.timestamp).toLocaleTimeString();
  console.log(\`[\${time}] [\${log.level.toUpperCase()}] \${log.message}\`);
};

es.onerror = () => {
  console.warn("Log stream disconnected, retrying...");
  // EventSource auto-reconnects after a short delay
};

// Clean up
// es.close();`} />
                    <p className="text-muted-foreground leading-7">
                        For authenticated runtimes (<code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">ROOTCX_AUTH=required</code>), the native <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">EventSource</code> API does not support <code className="rounded bg-white/5 px-1.5 py-0.5 font-mono text-xs text-foreground">Authorization</code> headers. Use a fetch-based SSE client or a server-side proxy instead:
                    </p>
                    <CodeBlock language="typescript" code={`// Authenticated SSE with fetch
const response = await fetch(
  "http://localhost:9100/api/v1/apps/crm/logs",
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
      console.log(log);
    }
  }
}`} />
                </section>

                <PageNav href="/modules/logs" />
            </div>
        </DocsLayout>
    );
}
