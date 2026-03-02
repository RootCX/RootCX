import { fetchCore } from "@/core/auth";

export interface WorkerInfo { appId: string; status: string }

interface State { isCoreAdmin: boolean; workers: WorkerInfo[]; loading: boolean; error: string | null }

let state: State = { isCoreAdmin: false, workers: [], loading: false, error: null };
const listeners = new Set<() => void>();
let snapshot = state;

function emit() { snapshot = { ...state }; listeners.forEach((fn) => fn()); }
function errMsg(e: unknown) { return e instanceof Error ? e.message : String(e); }

export const subscribe = (fn: () => void) => (listeners.add(fn), () => listeners.delete(fn));
export const getSnapshot = () => snapshot;

async function api<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetchCore(path, init);
  if (!res.ok) throw new Error(await res.text().catch(() => res.statusText));
  return res.json();
}

export async function checkAdmin() {
  try {
    const res = await fetchCore("/api/v1/apps/core/permissions");
    if (!res.ok) { state = { ...state, isCoreAdmin: false }; emit(); return; }
    const { permissions } = (await res.json()) as { permissions: string[] };
    state = { ...state, isCoreAdmin: permissions.includes("*") }; emit();
  } catch { state = { ...state, isCoreAdmin: false }; emit(); }
}

export async function refreshWorkers() {
  state = { ...state, loading: true, error: null }; emit();
  try {
    const { workers } = await api<{ workers: Record<string, string> }>("/api/v1/workers");
    state = { ...state, workers: Object.entries(workers).map(([appId, status]) => ({ appId, status })), loading: false };
    emit();
  } catch (e) { state = { ...state, loading: false, error: errMsg(e) }; emit(); }
}

async function withRefresh(fn: () => Promise<unknown>) {
  try { await fn(); await refreshWorkers(); }
  catch (e) { state = { ...state, error: errMsg(e) }; emit(); }
}

export const startWorker = (appId: string) =>
  withRefresh(() => api(`/api/v1/apps/${encodeURIComponent(appId)}/worker/start`, { method: "POST" }));

export const stopWorker = (appId: string) =>
  withRefresh(() => api(`/api/v1/apps/${encodeURIComponent(appId)}/worker/stop`, { method: "POST" }));
