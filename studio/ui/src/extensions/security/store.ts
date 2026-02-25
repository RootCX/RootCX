import { invoke } from "@tauri-apps/api/core";
import { fetchCore } from "@/core/auth";

export interface Role { name: string; description: string | null; inherits: string[] }
export interface User { id: string; username: string; email: string | null; displayName: string | null; createdAt: string }
export interface Assignment { userId: string; role: string; assignedAt: string }
export interface Policy { role: string; entity: string; actions: string[]; ownership: boolean }

interface State {
  appId: string | null;
  roles: Role[]; users: User[]; assignments: Assignment[]; policies: Policy[];
  loading: boolean; error: string | null; isAdmin: boolean;
}

let state: State = { appId: null, roles: [], users: [], assignments: [], policies: [], loading: false, error: null, isAdmin: false };
const listeners = new Set<() => void>();
let snapshot = state;

function emit() { snapshot = { ...state }; listeners.forEach((fn) => fn()); }
function errMsg(e: unknown) { return e instanceof Error ? e.message : String(e); }

export const subscribe = (fn: () => void) => (listeners.add(fn), () => listeners.delete(fn));
export const getSnapshot = () => snapshot;

async function get<T>(path: string, fallback?: T): Promise<T> {
  const res = await fetchCore(path);
  if (!res.ok) {
    if (fallback !== undefined && res.status === 403) return fallback;
    throw new Error(await res.text().catch(() => res.statusText));
  }
  return res.json();
}

function appPath(id: string) { return `/api/v1/apps/${encodeURIComponent(id)}`; }

async function fetchAll(appId: string) {
  const p = appPath(appId);
  const [roles, users, assignments, policies, perms] = await Promise.all([
    get<Role[]>(`${p}/roles`),
    get<User[]>("/api/v1/users"),
    get<Assignment[]>(`${p}/roles/assignments`, []),
    get<Policy[]>(`${p}/policies`),
    get<{ permissions: Record<string, { actions: string[] }> }>(`${p}/permissions`, { permissions: {} }),
  ]);
  return { roles, users, assignments, policies, isAdmin: perms.permissions["*"]?.actions.includes("*") ?? false };
}

export async function loadProject(projectPath: string) {
  state = { ...state, loading: true, error: null }; emit();
  try {
    const raw = await invoke<string>("read_file", { path: `${projectPath}/manifest.json` });
    const { appId } = JSON.parse(raw) as { appId: string };
    state = { ...state, appId, ...(await fetchAll(appId)), loading: false }; emit();
  } catch (e) {
    state = { ...state, loading: false, error: errMsg(e) }; emit();
  }
}

export async function refresh() {
  if (!state.appId) return;
  state = { ...state, loading: true, error: null }; emit();
  try {
    state = { ...state, ...(await fetchAll(state.appId)), loading: false }; emit();
  } catch (e) {
    state = { ...state, loading: false, error: errMsg(e) }; emit();
  }
}

async function mutate(action: "assign" | "revoke", userId: string, role: string) {
  if (!state.appId) return;
  try {
    const res = await fetchCore(`${appPath(state.appId)}/roles/${action}`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ userId, role }),
    });
    if (!res.ok) throw new Error(await res.text().catch(() => `${action} failed`));
    await refresh();
  } catch (e) {
    state = { ...state, error: errMsg(e) }; emit();
  }
}

export const assignRole = (userId: string, role: string) => mutate("assign", userId, role);
export const revokeRole = (userId: string, role: string) => mutate("revoke", userId, role);
