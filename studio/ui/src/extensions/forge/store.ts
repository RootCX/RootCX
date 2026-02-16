import {
  createOpencodeClient,
  type Session,
  type Message,
  type Part,
  type Permission,
  type Event,
  type Config,
} from "@opencode-ai/sdk";
import { invoke } from "@tauri-apps/api/core";

const client = createOpencodeClient({ baseUrl: "http://127.0.0.1:4096" });

export interface ProviderInfo {
  id: string;
  name: string;
  env: string[];
  models: Record<string, { id: string; name: string }>;
}

export interface ForgeState {
  connected: boolean;
  sessionId: string | null;
  sessions: Session[];
  messages: Message[];
  parts: Map<string, Part[]>;
  permissions: Permission[];
  streaming: boolean;
  error: string | null;
  providers: ProviderInfo[];
  connectedProviders: string[];
  currentConfig: Config | null;
}

type Listener = () => void;

let state: ForgeState = {
  connected: false,
  sessionId: null,
  sessions: [],
  messages: [],
  parts: new Map(),
  permissions: [],
  streaming: false,
  error: null,
  providers: [],
  connectedProviders: [],
  currentConfig: null,
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
    loadProviders();
    loadConfig();

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
      const perm = event.properties.permission;
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
        ...(state.sessionId === session.id
          ? { sessionId: null, messages: [], parts: new Map(), permissions: [] }
          : {}),
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
  state = { ...state, sessionId, messages: [], parts: new Map(), permissions: [], streaming: false, error: null };
  emit();
  try {
    const result = await client.session.messages({ path: { id: sessionId } });
    if (result.data) {
      state = { ...state, messages: result.data };
      emit();
    }
  } catch { /* ignore */ }
}

export async function createSession() {
  try {
    const result = await client.session.create();
    if (result.data) {
      const session = result.data;
      state = { ...state, sessionId: session.id, sessions: [session, ...state.sessions], messages: [], parts: new Map(), permissions: [], streaming: false, error: null };
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

export async function loadProviders() {
  try {
    const result = await client.provider.list();
    if (result.data) {
      state = {
        ...state,
        providers: result.data.all.map((p) => ({
          id: p.id,
          name: p.name,
          env: p.env,
          models: Object.fromEntries(
            Object.entries(p.models).map(([key, m]) => [key, { id: m.id, name: m.name }]),
          ),
        })),
        connectedProviders: result.data.connected,
      };
      emit();
    }
  } catch { /* ignore */ }
}

export async function loadConfig() {
  try {
    const result = await client.config.get();
    if (result.data) {
      state = { ...state, currentConfig: result.data };
      emit();
    }
  } catch { /* ignore */ }
}

let startedProject: string | null = null;

export async function startForProject(projectPath: string): Promise<void> {
  if (startedProject === projectPath) return;
  startedProject = projectPath;
  try {
    await invoke("start_forge", { projectPath });
  } catch {
    startedProject = null;
  }
}

connectEvents();
