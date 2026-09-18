import { useState, useRef, useEffect, useCallback } from "react";
import { AuthGate, useRuntimeClient } from "@rootcx/sdk";
import { Alert, AlertDescription, AlertTitle, Button, Textarea } from "@rootcx/ui";
import { ChatScrollArea } from "@/components/chat/chat-scroll-area";
import { Markdown } from "@/components/chat/markdown";
import { cn } from "@/lib/utils";
import { AuthForm, AuthLoading } from "@/components/auth-form";
import { IconLogout, IconArrowUp, IconSquareFilled, IconChevronDown } from "@tabler/icons-react";

const APP_ID = "__APP_ID__";

interface Message {
  id: string;
  role: "user" | "assistant";
  content: string;
  toolCalls: ToolActivity[];
  approvals: ApprovalRequest[];
  error?: string;
}

interface ToolActivity {
  id: string;
  name: string;
  input: unknown;
  status: "running" | "completed" | "error";
  output?: unknown;
  error?: string;
  durationMs?: number;
}

interface ApprovalRequest {
  approvalId: string;
  toolName: string;
  args: unknown;
  reason: string;
  resolved?: "approved" | "rejected";
}

interface SSEEvent { event: string; data: string }

function parseSSE(raw: string): { events: SSEEvent[]; remainder: string } {
  const events: SSEEvent[] = [];
  let currentEvent = "message";
  let currentData = "";
  const lines = raw.split("\n");
  let lastConsumed = -1;
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    if (line.startsWith("event: ")) currentEvent = line.slice(7).trim();
    else if (line.startsWith("data: ")) currentData += (currentData ? "\n" : "") + line.slice(6);
    else if (line === "") {
      if (currentData) events.push({ event: currentEvent, data: currentData });
      currentEvent = "message";
      currentData = "";
      lastConsumed = i;
    }
  }
  return { events, remainder: lastConsumed >= 0 ? lines.slice(lastConsumed + 1).join("\n") : raw };
}

function stringify(value: unknown): string {
  if (value === null || value === undefined) return "";
  if (typeof value === "string") return value;
  return JSON.stringify(value, null, 2);
}

const TOOL_LABELS: Record<string, string> = {
  query_data: "Querying", mutate_data: "Saving",
  web_search: "Searching", web_fetch: "Fetching",
};
const TOOL_STATUS_CLASSES: Record<ToolActivity["status"], string> = {
  running: "bg-warning animate-pulse",
  completed: "bg-success",
  error: "bg-destructive",
};

const FADE_MASK = "linear-gradient(to bottom, transparent 0%, black 12px, black calc(100% - 12px), transparent 100%)";

function toolTitle(tc: ToolActivity) {
  const label = TOOL_LABELS[tc.name] ?? tc.name;
  const inp = (typeof tc.input === "object" && tc.input !== null ? tc.input : {}) as Record<string, unknown>;
  const detail = (inp.url as string) ?? (inp.entity as string) ?? (inp.query as string) ?? null;
  return detail ? `${label} ${detail}` : label;
}

export default function App() {
  return (
    <AuthGate appTitle={APP_ID} renderForm={(props) => <AuthForm {...props} />} renderLoading={AuthLoading}>
      {({ user, logout }) => <Chat user={user} onLogout={logout} />}
    </AuthGate>
  );
}

function useCinematicScroll(ref: React.RefObject<HTMLDivElement | null>, enabled: boolean, deps: unknown[]) {
  const raf = useRef(0);
  useEffect(() => {
    const el = ref.current;
    if (!el || !enabled) return;
    const start = el.scrollTop;
    const distance = el.scrollHeight - el.clientHeight - start;
    if (distance <= 0) return;
    cancelAnimationFrame(raf.current);
    const dur = Math.min(1800, Math.max(800, distance * 12));
    let t0: number | null = null;
    const step = (now: number) => {
      if (!t0) t0 = now;
      const p = Math.min((now - t0) / dur, 1);
      el.scrollTop = start + distance * (1 - (1 - p) ** 4); // quartic ease-out
      if (p < 1) raf.current = requestAnimationFrame(step);
    };
    raf.current = requestAnimationFrame(step);
    return () => cancelAnimationFrame(raf.current);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);
}

function Composer({
  input, setInput, onSubmit, onAbort, streaming, liveTools, className,
}: {
  input: string; setInput: (v: string) => void;
  onSubmit: () => void; onAbort: () => void;
  streaming: boolean; liveTools: ToolActivity[];
  className?: string;
}) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const live = streaming && liveTools.length > 0;
  const lastTool = liveTools[liveTools.length - 1];

  useCinematicScroll(scrollRef, live, [liveTools.length, lastTool?.status]);

  return (
    <div className={cn("mx-auto w-full max-w-3xl", className)}>
      <div className={cn("flex flex-col gap-3 rounded-xl border bg-card p-3 shadow-md transition-colors", live ? "border-ring" : "border-border")}>
        {live ? (
          <div
            ref={scrollRef}
            className="max-h-[100px] overflow-y-hidden overflow-x-hidden pt-3"
            style={{ maskImage: FADE_MASK, WebkitMaskImage: FADE_MASK }}
          >
            {liveTools.map((tool, i) => (
              <div
                key={tool.id}
                className={cn("px-5 py-1 text-xs truncate text-muted-foreground transition-opacity duration-700",
                  tool.status === "completed" && i !== liveTools.length - 1 && "opacity-40")}
              >
                {toolTitle(tool)}
              </div>
            ))}
          </div>
        ) : (
          <Textarea
            rows={3}
            className="resize-none"
            aria-label="Message"
            placeholder={streaming ? "Thinking…" : "Ask anything…"}
            value={input}
            onChange={(e) => setInput(e.target.value)}
            disabled={streaming}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); onSubmit(); }
            }}
          />
        )}
        <div className="flex items-center justify-end">
          {streaming ? (
            <Button
              size="icon"
              variant="secondary"
              aria-label="Stop response"
              onClick={onAbort}
            >
              <IconSquareFilled />
            </Button>
          ) : (
            <Button
              size="icon"
              aria-label="Send message"
              disabled={!input.trim()}
              onClick={onSubmit}
            >
              <IconArrowUp />
            </Button>
          )}
        </div>
      </div>
    </div>
  );
}

function Chat({ user, onLogout }: { user: { email: string }; onLogout: () => void }) {
  const client = useRuntimeClient();
  const [messages, setMessages] = useState<Message[]>([]);
  const [input, setInput] = useState("");
  const [streaming, setStreaming] = useState(false);
  const [sessionId, setSessionId] = useState<string | null>(null);
  const abortRef = useRef<AbortController | null>(null);

  const authHeaders = useCallback((): Record<string, string> => {
    const token = client.getAccessToken();
    const h: Record<string, string> = { "Content-Type": "application/json" };
    if (token) h["Authorization"] = `Bearer ${token}`;
    return h;
  }, [client]);

  const apiUrl = useCallback(
    (path: string) => `${client.getBaseUrl()}/api/v1/apps/${APP_ID}/agent${path}`,
    [client],
  );

  const sendMessage = useCallback(async () => {
    const text = input.trim();
    if (!text || streaming) return;

    setInput("");
    setStreaming(true);
    const ctrl = new AbortController();
    abortRef.current = ctrl;

    const assistantId = crypto.randomUUID();
    const userMsg: Message = { id: crypto.randomUUID(), role: "user", content: text, toolCalls: [], approvals: [] };
    const assistantMsg: Message = { id: assistantId, role: "assistant", content: "", toolCalls: [], approvals: [] };
    setMessages((prev) => [...prev, userMsg, assistantMsg]);

    const update = (fn: (m: Message) => Message) =>
      setMessages((prev) => prev.map((m) => (m.id === assistantId ? fn(m) : m)));

    try {
      const res = await fetch(apiUrl("/invoke"), {
        method: "POST",
        headers: authHeaders(),
        body: JSON.stringify({ message: text, ...(sessionId && { session_id: sessionId }) }),
        signal: ctrl.signal,
      });

      if (!res.ok) {
        const body = await res.text().catch(() => "");
        throw new Error(`HTTP ${res.status}${body ? `: ${body}` : ""}`);
      }

      const reader = res.body!.getReader();
      const decoder = new TextDecoder();
      let buffer = "";

      while (true) {
        const { done, value } = await reader.read();
        if (done) break;
        buffer += decoder.decode(value, { stream: true });

        const { events, remainder } = parseSSE(buffer);
        buffer = remainder;

        for (const sse of events) {
          let data: Record<string, unknown>;
          try { data = JSON.parse(sse.data); } catch { continue; }

          switch (sse.event) {
            case "chunk":
              update((m) => ({ ...m, content: m.content + String(data.delta ?? "") }));
              break;
            case "done":
              if (data.session_id) setSessionId(data.session_id as string);
              break;
            case "error":
              update((m) => ({ ...m, error: String(data.error) }));
              break;
            case "tool_call_started":
              update((m) => ({
                ...m,
                toolCalls: [...m.toolCalls, {
                  id: data.call_id as string,
                  name: data.tool_name as string,
                  input: data.input,
                  status: "running",
                }],
              }));
              break;
            case "tool_call_completed":
              update((m) => ({
                ...m,
                toolCalls: m.toolCalls.map((tc) =>
                  tc.id === data.call_id
                    ? { ...tc, status: (data.error ? "error" : "completed") as ToolActivity["status"], output: data.output, error: data.error as string | undefined, durationMs: data.duration_ms as number | undefined }
                    : tc,
                ),
              }));
              break;
            case "approval_required":
              update((m) => ({
                ...m,
                approvals: [...m.approvals, {
                  approvalId: data.approval_id as string,
                  toolName: data.tool_name as string,
                  args: data.args,
                  reason: data.reason as string,
                }],
              }));
              break;
          }
        }
      }
    } catch (err) {
      if ((err as Error).name !== "AbortError")
        update((m) => ({ ...m, error: `Failed to reach agent: ${err}` }));
    } finally {
      abortRef.current = null;
      setStreaming(false);
    }
  }, [input, streaming, sessionId, authHeaders, apiUrl]);

  const abort = useCallback(() => {
    abortRef.current?.abort();
  }, []);

  const respondApproval = useCallback(
    async (approvalId: string, approved: boolean) => {
      setMessages((prev) =>
        prev.map((m) => ({
          ...m,
          approvals: m.approvals.map((a) =>
            a.approvalId === approvalId ? { ...a, resolved: approved ? "approved" as const : "rejected" as const } : a,
          ),
        })),
      );
      try {
        await fetch(apiUrl(`/approvals/${approvalId}`), {
          method: "POST",
          headers: authHeaders(),
          body: JSON.stringify({ action: approved ? "approve" : "reject" }),
        });
      } catch (err) {
        console.error("Approval request failed:", err);
      }
    },
    [authHeaders, apiUrl],
  );

  const items: React.ReactNode[] = [];
  let liveTools: ToolActivity[] = [];

  for (const msg of messages) {
    if (msg.role === "user") {
      items.push(
        <div key={msg.id} className="flex justify-end">
          <div className="max-w-[80%] rounded-2xl rounded-br-md bg-muted/80 px-4 py-2.5 text-[14px] leading-[1.7] whitespace-pre-wrap">
            {msg.content}
          </div>
        </div>,
      );
    } else {
      if (streaming && msg.toolCalls.length > 0) {
        liveTools = msg.toolCalls;
      } else if (msg.toolCalls.length > 0) {
        items.push(<MiniToolCard key={`tc-${msg.id}`} tools={msg.toolCalls} />);
      }

      if (msg.content || msg.error) {
        items.push(
          <div key={msg.id} className="w-full">
            {msg.content && <Markdown>{msg.content}</Markdown>}
            {msg.error && (
              <Alert variant="destructive" className="mt-2">
                <AlertDescription>{msg.error}</AlertDescription>
              </Alert>
            )}
          </div>,
        );
      }

      if (msg.approvals.length > 0) {
        items.push(
          <div key={`approvals-${msg.id}`} className="flex flex-col gap-2">
            {msg.approvals.map((a) => (
              <ApprovalCard key={a.approvalId} approval={a} onRespond={respondApproval} />
            ))}
          </div>,
        );
      }
    }
  }

  if (messages.length === 0) {
    return (
      <div className="flex h-[100dvh] flex-col bg-background text-foreground">
        <Header user={user} onLogout={onLogout} />
        <div className="flex flex-1 flex-col items-center justify-center px-4 sm:px-6">
          <div className="flex w-full max-w-3xl flex-col items-center gap-8">
            <div className="flex flex-col items-center gap-3">
              <h1 className="text-2xl font-medium tracking-tight text-foreground/80">What can I help you with?</h1>
              <p className="text-sm text-muted-foreground">Describe what you need. The agent will use its tools to get it done.</p>
            </div>
            <Composer input={input} setInput={setInput} onSubmit={sendMessage} onAbort={abort} streaming={streaming} liveTools={[]} />
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="flex h-[100dvh] flex-col bg-background text-foreground">
      <Header user={user} onLogout={onLogout} />
      <ChatScrollArea className="flex-1" contentClassName="mx-auto flex w-full max-w-3xl flex-col gap-5 px-4 py-4 sm:px-6 sm:py-6">
        {items}
      </ChatScrollArea>
      <div className="shrink-0 px-4 pb-[max(1rem,env(safe-area-inset-bottom))] pt-2 sm:px-6 sm:pb-5">
        <Composer input={input} setInput={setInput} onSubmit={sendMessage} onAbort={abort} streaming={streaming} liveTools={liveTools} />
      </div>
    </div>
  );
}

function Header({ user, onLogout }: { user: { email: string }; onLogout: () => void }) {
  return (
    <div className="flex h-12 shrink-0 items-center justify-between border-b border-border/60 px-5">
      <span className="text-sm font-semibold tracking-tight">{APP_ID}</span>
      <div className="flex items-center gap-2 text-xs text-muted-foreground">
        <span>{user.email}</span>
        <Button
          size="icon-sm"
          variant="toolbar"
          aria-label="Sign out"
          onClick={onLogout}
        >
          <IconLogout />
        </Button>
      </div>
    </div>
  );
}

function MiniToolCard({ tools }: { tools: ToolActivity[] }) {
  const [expanded, setExpanded] = useState(false);
  const errored = tools.some((t) => t.status === "error");

  return (
    <div className="-mt-3">
      <button
        type="button"
        onClick={() => setExpanded((e) => !e)}
        aria-expanded={expanded}
        className={cn("flex items-center gap-2 rounded-lg px-3 py-1.5 text-xs transition-colors",
          errored ? "text-destructive" : "text-muted-foreground hover:text-foreground")}
      >
        <span className={cn("size-1.5 rounded-full", errored ? "bg-destructive" : "bg-success")} />
        {tools.length} {tools.length === 1 ? "operation" : "operations"}
        <IconChevronDown className={cn("size-3 transition-transform duration-200", expanded && "rotate-180")} />
      </button>
      {expanded && (
        <div className="mt-1 max-h-[300px] overflow-y-auto rounded-xl border border-border/30 bg-card/50 py-0.5">
          {tools.map((tool) => (
            <ToolDetailView key={tool.id} tool={tool} />
          ))}
        </div>
      )}
    </div>
  );
}

function toolOutputSummary(tool: ToolActivity): string | null {
  if (tool.status === "running") return null;
  if (tool.error) return tool.error;
  const out = tool.output;
  if (out === null || out === undefined) return null;
  if (typeof out === "string") return out;

  if (Array.isArray(out)) {
    const count = out.length;
    return count === 0 ? "No results" : `${count} result${count > 1 ? "s" : ""} returned`;
  }
  if (typeof out === "object") {
    const o = out as Record<string, unknown>;
    if ("ok" in o) return o.ok ? `Done${o.id ? ` (id: ${o.id})` : ""}` : `Failed${o.error ? `: ${o.error}` : ""}`;
    if ("url" in o && "content" in o) return `Fetched ${o.url}`;
    const s = JSON.stringify(out);
    return s.length > 120 ? s.slice(0, 120) + "…" : s;
  }
  return String(out);
}

function ToolDetailView({ tool }: { tool: ToolActivity }) {
  const summary = toolOutputSummary(tool);

  return (
    <div>
      <div className="px-3 py-1 text-xs truncate text-muted-foreground flex items-center gap-2">
        <span className={cn("size-1.5 shrink-0 rounded-full", TOOL_STATUS_CLASSES[tool.status])} />
        {toolTitle(tool)}
        {tool.durationMs != null && <span className="ml-auto text-muted-foreground">{tool.durationMs}ms</span>}
      </div>
      {summary && (
        <div className="mx-3 mb-1.5 rounded-lg bg-muted/50 px-3 py-2 text-[11px] font-mono text-muted-foreground">
          <div className="max-h-[120px] overflow-y-auto whitespace-pre-wrap break-all leading-relaxed truncate">{summary}</div>
        </div>
      )}
    </div>
  );
}

function ApprovalCard({
  approval,
  onRespond,
}: {
  approval: ApprovalRequest;
  onRespond: (id: string, approved: boolean) => void;
}) {
  const resolved = approval.resolved;

  if (resolved) {
    return (
      <div className="flex items-center gap-2 rounded-lg px-3 py-1.5 text-xs text-muted-foreground">
        <span className={cn("size-1.5 rounded-full", resolved === "approved" ? "bg-success" : "bg-destructive")} />
        {approval.toolName} — {resolved}
      </div>
    );
  }

  return (
    <Alert>
      <AlertTitle>
        {approval.toolName}
      </AlertTitle>
      {approval.reason && (
        <AlertDescription>{approval.reason}</AlertDescription>
      )}
      <pre className="mb-3 max-h-[120px] overflow-auto rounded-lg bg-muted/50 px-3 py-2 font-mono text-[11px] text-muted-foreground">
        {stringify(approval.args)}
      </pre>
      <div className="flex gap-2">
        <Button
          size="sm"
          variant="outline"
          onClick={() => onRespond(approval.approvalId, true)}
        >
          Approve
        </Button>
        <Button
          size="sm"
          variant="ghost"
          onClick={() => onRespond(approval.approvalId, false)}
        >
          Deny
        </Button>
      </div>
    </Alert>
  );
}
