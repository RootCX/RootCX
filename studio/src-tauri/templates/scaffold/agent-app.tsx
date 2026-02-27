import { useState, useRef, useEffect, useCallback } from "react";
import { AuthGate, useRuntimeClient } from "@rootcx/runtime";
import { Button } from "@rootcx/ui";
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

const FADE_MASK = "linear-gradient(to bottom, transparent 0%, black 12px, black calc(100% - 12px), transparent 100%)";

function toolTitle(tc: ToolActivity) {
  const label = TOOL_LABELS[tc.name] ?? tc.name;
  const inp = (typeof tc.input === "object" && tc.input !== null ? tc.input : {}) as Record<string, unknown>;
  const detail = (inp.url as string) ?? (inp.entity as string) ?? (inp.query as string) ?? null;
  return detail ? `${label} ${detail}` : label;
}

export default function App() {
  return (
    <AuthGate appTitle={APP_ID}>
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
    <div className={`mx-auto w-full max-w-3xl ${className ?? ""}`}>
      <div className={`flex flex-col rounded-2xl border bg-card shadow-lg shadow-black/5 dark:shadow-black/20 transition-colors ${live ? "border-primary/30" : "border-border/60 focus-within:border-border"}`}>
        {live ? (
          <div
            ref={scrollRef}
            className="max-h-[100px] overflow-y-hidden overflow-x-hidden pt-3"
            style={{ maskImage: FADE_MASK, WebkitMaskImage: FADE_MASK }}
          >
            {liveTools.map((tool, i) => (
              <div
                key={tool.id}
                className={`px-5 py-1 text-xs truncate text-muted-foreground transition-opacity duration-700 ${
                  tool.status === "completed" && i !== liveTools.length - 1 ? "opacity-40" : ""
                }`}
              >
                {toolTitle(tool)}
              </div>
            ))}
          </div>
        ) : (
          <textarea
            rows={3}
            className="w-full resize-none bg-transparent px-5 pt-4 pb-1 text-[14px] leading-relaxed text-foreground placeholder:text-muted-foreground/40 focus:outline-none"
            placeholder={streaming ? "Thinking…" : "Ask anything…"}
            value={input}
            onChange={(e) => setInput(e.target.value)}
            disabled={streaming}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); onSubmit(); }
            }}
          />
        )}
        <div className="flex items-center justify-end px-3 pb-3">
          {streaming ? (
            <button
              className="flex h-8 w-8 items-center justify-center rounded-full bg-muted text-foreground transition-colors hover:bg-muted-foreground/20"
              onClick={onAbort}
            >
              <IconSquareFilled className="h-3 w-3" />
            </button>
          ) : (
            <button
              className={`flex h-8 w-8 items-center justify-center rounded-full transition-all ${
                input.trim() ? "bg-primary text-primary-foreground hover:bg-primary/90" : "bg-muted text-muted-foreground/30"
              }`}
              disabled={!input.trim()}
              onClick={onSubmit}
            >
              <IconArrowUp className="h-4 w-4" />
            </button>
          )}
        </div>
      </div>
    </div>
  );
}

function Chat({ user, onLogout }: { user: { username: string }; onLogout: () => void }) {
  const client = useRuntimeClient();
  const [messages, setMessages] = useState<Message[]>([]);
  const [input, setInput] = useState("");
  const [streaming, setStreaming] = useState(false);
  const [sessionId, setSessionId] = useState<string | null>(null);
  const abortRef = useRef<AbortController | null>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  const lastTop = useRef(0);
  const msgCount = useRef(0);

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const onScroll = () => {
      if (el.scrollTop < lastTop.current - 5) stick.current = false;
      else if (el.scrollHeight - el.scrollTop - el.clientHeight < 5) stick.current = true;
      lastTop.current = el.scrollTop;
    };
    el.addEventListener("scroll", onScroll, { passive: true });
    return () => el.removeEventListener("scroll", onScroll);
  }, []);

  useEffect(() => {
    if (messages.length > msgCount.current) stick.current = true;
    msgCount.current = messages.length;
    if (stick.current) scrollRef.current?.scrollTo(0, scrollRef.current.scrollHeight);
  }, [messages]);

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
          <div key={msg.id} className="w-full text-[14px] leading-[1.7]">
            {msg.content && <div className="whitespace-pre-wrap">{msg.content}</div>}
            {msg.error && (
              <div className="mt-2 rounded-xl border border-destructive/20 bg-destructive/5 px-4 py-2.5 text-sm text-destructive/80">
                {msg.error}
              </div>
            )}
          </div>,
        );
      }

      if (msg.approvals.length > 0) {
        items.push(
          <div key={`approvals-${msg.id}`} className="space-y-2">
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
      <div className="flex h-screen flex-col bg-background text-foreground">
        <Header user={user} onLogout={onLogout} />
        <div className="flex flex-1 flex-col items-center justify-center px-6">
          <div className="flex w-full max-w-3xl flex-col items-center gap-8">
            <div className="flex flex-col items-center gap-3">
              <h1 className="text-2xl font-medium tracking-tight text-foreground/80">What can I help you with?</h1>
              <p className="text-sm text-muted-foreground/50">Describe what you need. The agent will use its tools to get it done.</p>
            </div>
            <Composer input={input} setInput={setInput} onSubmit={sendMessage} onAbort={abort} streaming={streaming} liveTools={[]} />
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="flex h-screen flex-col bg-background text-foreground">
      <Header user={user} onLogout={onLogout} />
      <div ref={scrollRef} className="min-h-0 flex-1 overflow-y-auto overflow-x-hidden">
        <div className="mx-auto w-full max-w-3xl space-y-5 px-6 py-6">
          {items}
        </div>
      </div>
      <div className="shrink-0 px-6 pb-5 pt-2">
        <Composer input={input} setInput={setInput} onSubmit={sendMessage} onAbort={abort} streaming={streaming} liveTools={liveTools} />
      </div>
    </div>
  );
}

function Header({ user, onLogout }: { user: { username: string }; onLogout: () => void }) {
  return (
    <div className="flex h-12 shrink-0 items-center justify-between border-b border-border/60 px-5">
      <span className="text-sm font-semibold tracking-tight">{APP_ID}</span>
      <div className="flex items-center gap-2 text-xs text-muted-foreground">
        <span>{user.username}</span>
        <button
          className="flex h-7 w-7 items-center justify-center rounded-lg text-muted-foreground/50 transition-colors hover:bg-muted hover:text-foreground"
          onClick={onLogout}
        >
          <IconLogout className="h-3.5 w-3.5" />
        </button>
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
        className={`flex items-center gap-2 rounded-lg px-3 py-1.5 text-xs transition-colors ${
          errored ? "text-destructive/60 hover:text-destructive/80" : "text-muted-foreground/50 hover:text-muted-foreground/70"
        }`}
      >
        <span className={`h-1.5 w-1.5 rounded-full ${errored ? "bg-destructive" : "bg-emerald-500"}`} />
        {tools.length} {tools.length === 1 ? "operation" : "operations"}
        <IconChevronDown className={`h-3 w-3 transition-transform duration-200 ${expanded ? "rotate-180" : ""}`} />
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
      <div className="px-3 py-1 text-xs truncate text-muted-foreground/60 flex items-center gap-2">
        <span className={`h-1.5 w-1.5 shrink-0 rounded-full ${
          tool.status === "running" ? "bg-yellow-500 animate-pulse" : tool.status === "completed" ? "bg-emerald-500" : "bg-destructive"
        }`} />
        {toolTitle(tool)}
        {tool.durationMs != null && <span className="ml-auto text-muted-foreground/40">{tool.durationMs}ms</span>}
      </div>
      {summary && (
        <div className="mx-3 mb-1.5 rounded-lg bg-muted/50 px-3 py-2 text-[11px] font-mono text-muted-foreground/60">
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
  const PERM_BTN = "h-8 rounded-lg text-xs";

  if (resolved) {
    return (
      <div className="flex items-center gap-2 rounded-lg px-3 py-1.5 text-xs text-muted-foreground/50">
        <span className={`h-1.5 w-1.5 rounded-full ${resolved === "approved" ? "bg-emerald-500" : "bg-destructive"}`} />
        {approval.toolName} — {resolved}
      </div>
    );
  }

  return (
    <div className="rounded-xl border border-yellow-500/20 bg-yellow-500/5 px-5 py-4">
      <div className="mb-2 text-sm font-medium text-yellow-700 dark:text-yellow-200/90">
        {approval.toolName}
      </div>
      {approval.reason && (
        <div className="mb-2 text-xs text-muted-foreground/60">{approval.reason}</div>
      )}
      <pre className="mb-3 max-h-[120px] overflow-auto rounded-lg bg-muted/50 px-3 py-2 font-mono text-[11px] text-muted-foreground/60">
        {stringify(approval.args)}
      </pre>
      <div className="flex gap-2">
        <Button
          size="sm"
          variant="outline"
          className={`${PERM_BTN} border-yellow-500/20 hover:bg-yellow-500/10`}
          onClick={() => onRespond(approval.approvalId, true)}
        >
          Approve
        </Button>
        <Button
          size="sm"
          variant="ghost"
          className={`${PERM_BTN} text-muted-foreground hover:text-foreground`}
          onClick={() => onRespond(approval.approvalId, false)}
        >
          Deny
        </Button>
      </div>
    </div>
  );
}
