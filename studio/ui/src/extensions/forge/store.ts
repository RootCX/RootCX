import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export interface Session { id: string; title: string; directory: string; summary_message_id: string | null; created_at: string; updated_at: string }
export interface Message { id: string; session_id: string; role: "user" | "assistant"; error: { name?: string; message?: string } | null; created_at: string; completed_at: string | null }
export interface Part { id: string; message_id: string; part_type: string; content: string; tool_name: string | null; tool_state: { status: string; title?: string } | null; tool_input: Record<string, unknown> | null; created_at: string }
export interface Permission { id: string; session_id: string; tool: string; title: string; description: string }
export interface QuestionOption { label: string; description: string }
export interface QuestionInfo { question: string; header: string; options: QuestionOption[]; multiple?: boolean; custom?: boolean }
export interface QuestionRequest { id: string; session_id: string; questions: QuestionInfo[] }

export interface ForgeState {
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

const EMPTY = {
  sessionId: null as string | null,
  messages: [] as Message[],
  parts: new Map<string, Part[]>(),
  permissions: [] as Permission[],
  questions: [] as QuestionRequest[],
  streaming: false,
  error: null as string | null,
};

let state: ForgeState = { ...EMPTY, sessions: [] };
const listeners = new Set<Listener>();
let snapshot = state;

function emit() {
  snapshot = { ...state, parts: new Map(state.parts) };
  listeners.forEach((fn) => fn());
}

export const subscribe = (fn: Listener) => { listeners.add(fn); return () => listeners.delete(fn); };
export const getSnapshot = () => snapshot;

// --- Events ---

listen<{ info: Message; parts?: Part[] }>("forge://message-updated", (e) => {
  const { info, parts: msgParts } = e.payload;
  if (info.session_id !== state.sessionId) return;
  const idx = state.messages.findIndex((m) => m.id === info.id);
  const messages = idx >= 0 ? state.messages.map((m, i) => (i === idx ? info : m)) : [...state.messages, info];
  const parts = msgParts ? new Map(state.parts).set(info.id, msgParts) : state.parts;
  state = { ...state, messages, parts, streaming: info.role === "assistant" && !info.completed_at && !info.error };
  emit();
});

listen<{ part: Part }>("forge://part-updated", (e) => {
  const part = e.payload.part;
  if (!state.messages.some((m) => m.id === part.message_id)) return;
  const parts = new Map(state.parts);
  const arr = parts.get(part.message_id) || [];
  const i = arr.findIndex((p) => p.id === part.id);
  parts.set(part.message_id, i >= 0 ? arr.map((p, j) => (j === i ? part : p)) : [...arr, part]);
  state = { ...state, parts };
  emit();
});

listen<Permission>("forge://permission-updated", (e) => {
  if (e.payload.session_id === state.sessionId) { state = { ...state, permissions: [...state.permissions, e.payload] }; emit(); }
});

listen<{ permissionID: string }>("forge://permission-replied", (e) => {
  state = { ...state, permissions: state.permissions.filter((p) => p.id !== e.payload.permissionID) }; emit();
});

listen<QuestionRequest>("forge://question-asked", (e) => {
  if (e.payload.session_id === state.sessionId) { state = { ...state, questions: [...state.questions, e.payload] }; emit(); }
});

for (const evt of ["forge://question-replied", "forge://question-rejected"] as const) {
  listen<{ requestID: string }>(evt, (e) => {
    state = { ...state, questions: state.questions.filter((q) => q.id !== e.payload.requestID) }; emit();
  });
}

listen<{ session: Session }>("forge://session-updated", (e) => {
  state = { ...state, sessions: state.sessions.map((s) => (s.id === e.payload.session.id ? e.payload.session : s)) };
  emit();
});

listen<{ sessionID: string }>("forge://session-idle", (e) => {
  if (e.payload.sessionID === state.sessionId) { state = { ...state, streaming: false }; emit(); }
});

listen<{ error: string }>("forge://error", (e) => {
  state = { ...state, error: e.payload.error, streaming: false }; emit();
});

// --- Actions ---

export async function loadSessions() {
  try { state = { ...state, sessions: await invoke<Session[]>("forge_list_sessions") }; emit(); } catch {}
}

export async function selectSession(sessionId: string) {
  state = { ...state, ...EMPTY, sessionId };
  emit();
  try {
    const data = await invoke<{ info: Message; parts: Part[] }[]>("forge_get_messages", { sessionId });
    state = { ...state, messages: data.map((m) => m.info), parts: new Map(data.map((m) => [m.info.id, m.parts] as [string, Part[]])) };
    emit();
  } catch {}
}

export async function createSession() {
  try {
    const s = await invoke<Session>("forge_create_session");
    state = { ...state, ...EMPTY, sessionId: s.id, sessions: [s, ...state.sessions] };
    emit();
    return s.id;
  } catch (e) {
    state = { ...state, error: e instanceof Error ? e.message : String(e) }; emit(); return null;
  }
}

export async function sendMessage(prompt: string) {
  const sid = state.sessionId ?? await createSession();
  if (!sid) return;
  state = { ...state, streaming: true, error: null }; emit();
  try { await invoke("forge_send_message", { sessionId: sid, text: prompt }); }
  catch (e) { state = { ...state, streaming: false, error: e instanceof Error ? e.message : String(e) }; emit(); }
}

export async function abortSession() {
  if (!state.sessionId) return;
  invoke("forge_abort", { sessionId: state.sessionId }).catch(() => {});
  state = { ...state, streaming: false }; emit();
}

export function replyPermission(id: string, response: "once" | "always" | "reject") {
  const perm = state.permissions.find((p) => p.id === id);
  invoke("forge_reply_permission", { id, sessionId: state.sessionId, tool: perm?.tool ?? "", response }).catch(() => {});
}

export function replyQuestion(id: string, answers: string[][]) {
  invoke("forge_reply_question", { id, answers }).catch(() => {});
  state = { ...state, questions: state.questions.filter((q) => q.id !== id) }; emit();
}

export function rejectQuestion(id: string) {
  invoke("forge_reject_question", { id }).catch(() => {});
  state = { ...state, questions: state.questions.filter((q) => q.id !== id) }; emit();
}

export function setCwd(path: string) { invoke("forge_set_cwd", { path }).catch(() => {}); }
export function reloadConfig() { invoke("forge_reload_config").catch(() => {}); }

loadSessions();
