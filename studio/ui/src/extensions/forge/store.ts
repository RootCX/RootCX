const FORGE_BASE = "http://127.0.0.1:3100";
const PROJECT_ID = "studio-default";

export type ForgePhase =
  | "idle" | "analyzing" | "planning" | "executing"
  | "verifying" | "done" | "error" | "stopped";

export interface ForgeToolCall { name: string; args: Record<string, unknown> }
export interface ForgeFileChange { path: string; action: "create" | "update" | "delete" }
export interface ForgeMessage { id: string; role: "user" | "assistant" | "status"; content: string; timestamp: number }

interface ForgeState {
  messages: ForgeMessage[];
  phase: ForgePhase;
  thinking: string;
  toolCalls: ForgeToolCall[];
  files: ForgeFileChange[];
  errors: string[];
  isStreaming: boolean;
  conversationId: string | null;
}

type Listener = () => void;

let state: ForgeState = {
  messages: [], phase: "idle", thinking: "", toolCalls: [],
  files: [], errors: [], isStreaming: false, conversationId: null,
};
const listeners = new Set<Listener>();
let snapshot = state;
let thinkingBuffer = "";
let eventSource: EventSource | null = null;

function emit() {
  snapshot = { ...state };
  listeners.forEach((fn) => fn());
}

function pushStatus(id: string, content: string) {
  state = { ...state, messages: [...state.messages, { id, role: "status", content, timestamp: Date.now() }] };
  emit();
}

export function subscribe(fn: Listener): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

export function getSnapshot(): ForgeState {
  return snapshot;
}

function connectSSE() {
  if (eventSource) eventSource.close();
  const es = new EventSource(`${FORGE_BASE}/stream/${PROJECT_ID}`);
  eventSource = es;

  es.addEventListener("phase", (e) => {
    state = { ...state, phase: JSON.parse(e.data).phase };
    emit();
  });

  es.addEventListener("agent_thinking", (e) => {
    thinkingBuffer += JSON.parse(e.data).content;
    state = { ...state, thinking: thinkingBuffer };
    emit();
  });

  es.addEventListener("tool_calls", (e) => {
    state = { ...state, toolCalls: JSON.parse(e.data).calls || [] };
    emit();
  });

  es.addEventListener("tool_executing", (e) =>
    pushStatus(`tool-${Date.now()}`, `Running ${JSON.parse(e.data).name}...`));

  es.addEventListener("tool_result", (e) => {
    const d = JSON.parse(e.data);
    pushStatus(`result-${Date.now()}`, `${d.name}: ${d.output}`);
  });

  es.addEventListener("status", (e) =>
    pushStatus(`status-${Date.now()}`, JSON.parse(e.data).message));

  es.addEventListener("error", (e) => {
    state = { ...state, errors: [...state.errors, JSON.parse((e as MessageEvent).data).message] };
    emit();
  });

  es.addEventListener("complete", (e) => {
    const data = JSON.parse(e.data);
    const messages = thinkingBuffer
      ? [...state.messages, { id: `assistant-${Date.now()}`, role: "assistant" as const, content: thinkingBuffer, timestamp: Date.now() }]
      : state.messages;
    thinkingBuffer = "";
    state = { ...state, messages, isStreaming: false, phase: data.success ? "done" : "error", files: data.applied_changes || [], thinking: "" };
    emit();
    es.close();
    eventSource = null;
  });

  es.onerror = () => {
    if (state.phase === "idle") { es.close(); eventSource = null; }
  };
}

export async function sendMessage(prompt: string, projectPath: string, appId?: string) {
  thinkingBuffer = "";
  state = {
    ...state, isStreaming: true, phase: "analyzing", thinking: "", toolCalls: [], errors: [],
    messages: [...state.messages, { id: `user-${Date.now()}`, role: "user", content: prompt, timestamp: Date.now() }],
  };
  emit();
  connectSSE();

  try {
    const resp = await fetch(`${FORGE_BASE}/chat`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ project_id: PROJECT_ID, project_path: projectPath, prompt, conversation_id: state.conversationId, app_id: appId || "" }),
    });
    if (!resp.ok) throw new Error(`Forge API error: ${resp.status}`);
    state = { ...state, conversationId: (await resp.json()).conversation_id };
    emit();
  } catch (err) {
    state = { ...state, isStreaming: false, phase: "error", errors: [...state.errors, err instanceof Error ? err.message : String(err)] };
    emit();
  }
}

export async function stopBuild() {
  try {
    await fetch(`${FORGE_BASE}/stop`, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ project_id: PROJECT_ID }) });
  } catch { /* best-effort */ }
  state = { ...state, isStreaming: false, phase: "stopped" };
  emit();
}
