import {
  createOpencodeClient,
  type Session,
  type Message,
  type Part,
  type Permission,
  type Event,
} from "@opencode-ai/sdk/client";
import { invoke } from "@tauri-apps/api/core";

let BASE_URL = "http://127.0.0.1:4096";
let client = createOpencodeClient({ baseUrl: BASE_URL });

export function setForgePort(port: number) {
  BASE_URL = `http://127.0.0.1:${port}`;
  client = createOpencodeClient({ baseUrl: BASE_URL });
}

export interface QuestionOption {
  label: string;
  description: string;
}

export interface QuestionInfo {
  question: string;
  header: string;
  options: QuestionOption[];
  multiple?: boolean;
  custom?: boolean;
}

export interface QuestionRequest {
  id: string;
  sessionID: string;
  questions: QuestionInfo[];
  tool?: { messageID: string; callID: string };
}

export interface ForgeState {
  connected: boolean;
  sessionId: string | null;
  sessions: Session[];
  messages: Message[];
  parts: Map<string, Part[]>;
  permissions: Permission[];
  questions: QuestionRequest[];
  streaming: boolean;
  error: string | null;
}

type Listener = () => void;

const EMPTY_SESSION = {
  sessionId: null as string | null,
  messages: [] as Message[],
  parts: new Map<string, Part[]>(),
  permissions: [] as Permission[],
  questions: [] as QuestionRequest[],
  streaming: false,
  error: null as string | null,
};

let state: ForgeState = {
  connected: false,
  sessionId: null,
  sessions: [],
  messages: [],
  parts: new Map(),
  permissions: [],
  questions: [],
  streaming: false,
  error: null,
};
const listeners = new Set<Listener>();
let snapshot = state;
let eventStream: AsyncGenerator | null = null;

function emit() {
  snapshot = { ...state, parts: new Map(state.parts) };
  listeners.forEach((fn) => fn());
}

export function subscribe(fn: Listener): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

export function getSnapshot(): ForgeState {
  return snapshot;
}

// ── SSE event loop ──

async function connectEvents() {
  if (eventStream) return;
  try {
    const result = await client.event.subscribe({ query: {} });
    eventStream = result.stream;
    state = { ...state, connected: true };
    emit();

    loadSessions();

    for await (const event of eventStream) {
      handleEvent(event as Event);
    }
  } catch {
    // stream ended or connection failed
  } finally {
    eventStream = null;
    state = { ...state, connected: false };
    emit();
    setTimeout(connectEvents, 3000);
  }
}

function handleEvent(event: Event) {
  switch (event.type) {
    case "message.updated": {
      const msg = event.properties.info;
      if (msg.sessionID !== state.sessionId) break;
      const idx = state.messages.findIndex((m) => m.id === msg.id);
      const messages =
        idx >= 0
          ? state.messages.map((m, i) => (i === idx ? msg : m))
          : [...state.messages, msg];
      state = {
        ...state,
        messages,
        streaming: msg.role === "assistant" && !msg.time.completed && !msg.error,
      };
      emit();
      break;
    }
    case "message.removed": {
      if (event.properties.sessionID !== state.sessionId) break;
      state = {
        ...state,
        messages: state.messages.filter((m) => m.id !== event.properties.messageID),
      };
      emit();
      break;
    }
    case "message.part.updated": {
      const part = event.properties.part;
      if (part.sessionID !== state.sessionId) break;
      const parts = new Map(state.parts);
      const existing = parts.get(part.messageID) || [];
      const partIdx = existing.findIndex((p) => p.id === part.id);
      if (partIdx >= 0) {
        const updated = [...existing];
        updated[partIdx] = part;
        parts.set(part.messageID, updated);
      } else {
        parts.set(part.messageID, [...existing, part]);
      }
      state = { ...state, parts };
      emit();
      break;
    }
    case "message.part.removed": {
      const { messageID, partID } = event.properties;
      if (!state.parts.has(messageID)) break;
      const parts = new Map(state.parts);
      parts.set(messageID, (parts.get(messageID) || []).filter((p) => p.id !== partID));
      state = { ...state, parts };
      emit();
      break;
    }
    case "permission.updated": {
      const perm = event.properties;
      if (perm.sessionID !== state.sessionId) break;
      state = { ...state, permissions: [...state.permissions, perm] };
      emit();
      break;
    }
    case "permission.replied": {
      state = {
        ...state,
        permissions: state.permissions.filter((p) => p.id !== event.properties.permissionID),
      };
      emit();
      break;
    }
    case "session.updated": {
      const session = event.properties.info;
      state = { ...state, sessions: state.sessions.map((s) => (s.id === session.id ? session : s)) };
      emit();
      break;
    }
    case "session.created": {
      state = { ...state, sessions: [event.properties.info, ...state.sessions] };
      emit();
      break;
    }
    case "session.deleted": {
      const session = event.properties.info;
      state = {
        ...state,
        sessions: state.sessions.filter((s) => s.id !== session.id),
        ...(state.sessionId === session.id ? EMPTY_SESSION : {}),
      };
      emit();
      break;
    }
    case "session.idle": {
      if (event.properties.sessionID === state.sessionId) {
        state = { ...state, streaming: false };
        emit();
      }
      break;
    }
    default: {
      // Question events (v2 API, not typed in SDK Event union)
      const { type, properties } = event as { type: string; properties: Record<string, unknown> };
      if (type === "question.asked") {
        const q = properties as unknown as QuestionRequest;
        if (q.sessionID === state.sessionId) {
          state = { ...state, questions: [...state.questions, q] };
          emit();
        }
      } else if (type === "question.replied" || type === "question.rejected") {
        state = { ...state, questions: state.questions.filter((q) => q.id !== properties.requestID) };
        emit();
      }
    }
  }
}

// ── API actions ──

export async function loadSessions() {
  try {
    const result = await client.session.list();
    if (result.data) {
      state = { ...state, sessions: result.data };
      emit();
    }
  } catch { /* ignore */ }
}

export async function selectSession(sessionId: string) {
  state = { ...state, ...EMPTY_SESSION, sessionId };
  emit();
  try {
    const result = await client.session.messages({ path: { id: sessionId } });
    if (result.data) {
      const messages = result.data.map((m) => m.info);
      const parts = new Map(result.data.map((m) => [m.info.id, m.parts]));
      state = { ...state, messages, parts };
      emit();
    }
  } catch { /* ignore */ }
}

export async function createSession() {
  try {
    const result = await client.session.create();
    if (result.data) {
      const session = result.data;
      state = { ...state, ...EMPTY_SESSION, sessionId: session.id, sessions: [session, ...state.sessions] };
      emit();
      return session.id;
    }
  } catch (err) {
    state = { ...state, error: err instanceof Error ? err.message : String(err) };
    emit();
  }
  return null;
}

export async function sendMessage(prompt: string) {
  let sessionId = state.sessionId;
  if (!sessionId) {
    sessionId = await createSession();
    if (!sessionId) return;
  }
  state = { ...state, streaming: true, error: null };
  emit();
  try {
    await client.session.promptAsync({
      path: { id: sessionId },
      body: { parts: [{ type: "text", text: prompt }] },
    });
  } catch (err) {
    state = { ...state, streaming: false, error: err instanceof Error ? err.message : String(err) };
    emit();
  }
}

export async function abortSession() {
  if (!state.sessionId) return;
  try { await client.session.abort({ path: { id: state.sessionId } }); } catch { /* best-effort */ }
  state = { ...state, streaming: false };
  emit();
}

export async function replyPermission(permissionId: string, response: "once" | "always" | "reject") {
  if (!state.sessionId) return;
  try {
    await client.postSessionIdPermissionsPermissionId({
      path: { id: state.sessionId, permissionID: permissionId },
      body: { response },
    });
  } catch { /* best-effort */ }
}

function dismissQuestion(requestId: string) {
  state = { ...state, questions: state.questions.filter((q) => q.id !== requestId) };
  emit();
}

export async function replyQuestion(requestId: string, answers: string[][]) {
  try {
    await fetch(`${BASE_URL}/question/${requestId}/reply`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ answers }),
    });
  } catch { /* best-effort */ }
  dismissQuestion(requestId);
}

export async function rejectQuestion(requestId: string) {
  try {
    await fetch(`${BASE_URL}/question/${requestId}/reject`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
    });
  } catch { /* best-effort */ }
  dismissQuestion(requestId);
}

let startedProject: string | null = null;

export async function startForProject(projectPath: string): Promise<void> {
  if (startedProject === projectPath) return;
  startedProject = projectPath;
  try {
    await invoke("start_forge", { projectPath });
    const status = await invoke<{ port?: number }>("get_forge_status");
    if (status.port) {
      setForgePort(status.port);
      if (eventStream) {
        eventStream = null;
        connectEvents();
      }
    }
  } catch {
    startedProject = null;
  }
}

// Resolve the real Forge port before first connection attempt
(async () => {
  try {
    const status = await invoke<{ port?: number }>("get_forge_status");
    if (status.port) {
      setForgePort(status.port);
    }
  } catch {
    // Forge may not be running yet — connectEvents will retry
  }
  connectEvents();
})();
