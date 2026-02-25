import type { AgentInfo, AgentMessage } from "@/types";

const BASE = "http://localhost:9100";

interface AgentChatState {
  messages: AgentMessage[];
  streaming: boolean;
  streamedText: string;
  sessionId: string | null;
  error: string | null;
}

interface State {
  agents: AgentInfo[];
  chats: Record<string, AgentChatState>;
}

let state: State = { agents: [], chats: {} };
const listeners = new Set<() => void>();
let snapshot = state;

function emit() {
  snapshot = { ...state, chats: { ...state.chats } };
  listeners.forEach((fn) => fn());
}

export const subscribe = (fn: () => void) => (listeners.add(fn), () => listeners.delete(fn));
export const getSnapshot = () => snapshot;

function chatFor(appId: string): AgentChatState {
  return (state.chats[appId] ??= {
    messages: [], streaming: false, streamedText: "", sessionId: null, error: null,
  });
}

function patchChat(appId: string, partial: Partial<AgentChatState>) {
  const cur = state.chats[appId];
  if (!cur) return;
  state = { ...state, chats: { ...state.chats, [appId]: { ...cur, ...partial } } };
  emit();
}

// ── Agent discovery ──

async function fetchAgents() {
  try {
    const res = await fetch(`${BASE}/api/v1/apps`);
    if (!res.ok) return;
    const apps: { id: string }[] = await res.json();

    const results = await Promise.allSettled(
      apps.map((a) => fetch(`${BASE}/api/v1/apps/${a.id}/agent`).then((r) => r.ok ? r.json() : null)),
    );
    const agents: AgentInfo[] = results
      .filter((r): r is PromiseFulfilledResult<AgentInfo> => r.status === "fulfilled" && r.value !== null)
      .map((r) => ({ app_id: r.value.app_id, name: r.value.name, description: r.value.description ?? null }));

    state = { ...state, agents };
    emit();
  } catch (e) { console.warn("fetchAgents failed:", e); }
}

let pollingId: ReturnType<typeof setInterval> | null = null;
export function startPolling() {
  if (pollingId) return;
  fetchAgents();
  pollingId = setInterval(fetchAgents, 5_000);
}
export function stopPolling() {
  if (pollingId) { clearInterval(pollingId); pollingId = null; }
}

// ── Worker lifecycle ──

const runningWorkers = new Set<string>();

async function ensureWorker(appId: string) {
  if (runningWorkers.has(appId)) return;
  try {
    const r = await fetch(`${BASE}/api/v1/apps/${appId}/worker/status`);
    if (r.ok && (await r.json()).status === "running") { runningWorkers.add(appId); return; }
  } catch (e) { console.warn("ensureWorker status check failed:", e); }

  const r = await fetch(`${BASE}/api/v1/apps/${appId}/worker/start`, { method: "POST" });
  if (!r.ok) throw new Error(await r.text().catch(() => "failed to start worker"));
  runningWorkers.add(appId);
  const WORKER_INIT_GRACE_MS = 1_000;
  await new Promise((resolve) => setTimeout(resolve, WORKER_INIT_GRACE_MS));
}

// ── SSE streaming ──

const abortControllers = new Map<string, AbortController>();

export async function sendAgentMessage(appId: string, message: string) {
  const chat = chatFor(appId);
  patchChat(appId, {
    messages: [...chat.messages, { role: "user", content: message }],
    streaming: true, streamedText: "", error: null,
  });

  const ctrl = new AbortController();
  abortControllers.set(appId, ctrl);

  try {
    await ensureWorker(appId);

    const body: Record<string, string> = { message };
    if (chat.sessionId) body.session_id = chat.sessionId;

    const res = await fetch(`${BASE}/api/v1/apps/${appId}/agent/invoke`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
      signal: ctrl.signal,
    });

    if (!res.ok || !res.body) {
      patchChat(appId, { streaming: false, error: await res.text().catch(() => "request failed") });
      return;
    }

    await readSSE(res.body, appId);
    finalize(appId);
  } catch (err) {
    if ((err as Error).name !== "AbortError")
      patchChat(appId, { streaming: false, error: err instanceof Error ? err.message : String(err) });
  } finally {
    abortControllers.delete(appId);
  }
}

async function readSSE(body: ReadableStream<Uint8Array>, appId: string) {
  const reader = body.getReader();
  const decoder = new TextDecoder();
  let buf = "", eventType = "";

  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    buf += decoder.decode(value, { stream: true });
    const lines = buf.split("\n");
    buf = lines.pop() ?? "";

    for (const line of lines) {
      if (line.startsWith("event:")) eventType = line.slice(6).trim();
      else if (line.startsWith("data:")) {
        try { handleSSE(appId, eventType, JSON.parse(line.slice(5))); } catch { /* skip */ }
      }
    }
  }
}

function handleSSE(appId: string, event: string, data: Record<string, unknown>) {
  const chat = state.chats[appId];
  if (!chat) return;
  const sid = (data.session_id as string) || chat.sessionId;

  switch (event) {
    case "chunk":
      patchChat(appId, { streamedText: chat.streamedText + ((data.delta as string) ?? ""), sessionId: sid });
      break;
    case "done": {
      const text = (data.response as string) ?? chat.streamedText;
      patchChat(appId, {
        messages: [...chat.messages, { role: "assistant", content: text }],
        streaming: false, streamedText: "", sessionId: sid,
      });
      break;
    }
    case "error":
      patchChat(appId, { streaming: false, error: (data.error as string) ?? "Unknown error", streamedText: "" });
      break;
  }
}

function finalize(appId: string) {
  const c = state.chats[appId];
  if (!c?.streaming) return;
  if (c.streamedText)
    patchChat(appId, { messages: [...c.messages, { role: "assistant", content: c.streamedText }], streaming: false, streamedText: "" });
  else
    patchChat(appId, { streaming: false });
}

export function abortAgent(appId: string) {
  abortControllers.get(appId)?.abort();
  patchChat(appId, { streaming: false });
}

export function clearChat(appId: string) {
  const { [appId]: _, ...rest } = state.chats;
  state = { ...state, chats: rest };
  emit();
}
