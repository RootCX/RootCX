import type { AgentMessage } from "@/types";
import { dismiss } from "@/core/notifications";
import { fetchCore } from "@/core/auth";

interface AgentChatState {
  messages: AgentMessage[];
  streaming: boolean;
  streamedText: string;
  sessionId: string | null;
  error: string | null;
}

interface State {
  chats: Record<string, AgentChatState>;
  deployed: Record<string, boolean>;
}

let state: State = { chats: {}, deployed: {} };
const listeners = new Set<() => void>();
let snapshot = state;

function emit() {
  snapshot = { ...state };
  listeners.forEach((fn) => fn());
}

export const subscribe = (fn: () => void) => (listeners.add(fn), () => listeners.delete(fn));
export const getSnapshot = () => snapshot;

export async function checkDeployment(appId: string): Promise<boolean> {
  let ok = false;
  try {
    const r = await fetchCore(`/api/v1/apps/${appId}/worker/status`);
    ok = r.ok && (await r.json()).status === "running";
  } catch {}
  state = { ...state, deployed: { ...state.deployed, [appId]: ok } };
  emit();
  if (ok) dismiss("agent-not-deployed");
  return ok;
}

export function markUndeployed() {
  state = { ...state, deployed: {} };
  emit();
}

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
    const body: Record<string, string> = { message };
    if (chat.sessionId) body.session_id = chat.sessionId;

    const res = await fetchCore(`/api/v1/apps/${appId}/agent/invoke`, {
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
        try { handleSSE(appId, eventType, JSON.parse(line.slice(5))); } catch {}
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

export async function uninstallAgent(appId: string) {
  const res = await fetchCore(`/api/v1/apps/${appId}`, { method: "DELETE" });
  if (!res.ok) throw new Error(await res.text().catch(() => "uninstall failed"));
  abortControllers.get(appId)?.abort();
  const { [appId]: _, ...chats } = state.chats;
  const { [appId]: __, ...deployed } = state.deployed;
  state = { ...state, chats, deployed };
  emit();
}
