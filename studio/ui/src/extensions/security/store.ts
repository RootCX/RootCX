import { invoke } from "@tauri-apps/api/core";
import { fetchCore } from "@/core/auth";

export interface Role { name: string; description: string | null; inherits: string[]; permissions: string[] }
export interface User { id: string; username: string; email: string | null; displayName: string | null; createdAt: string }
export interface Assignment { userId: string; role: string; assignedAt: string }
export interface PermissionDeclaration { key: string; description: string }

interface State {
  appId: string | null;
  roles: Role[]; users: User[]; assignments: Assignment[];
  availablePermissions: PermissionDeclaration[];
  loading: boolean; error: string | null; isAdmin: boolean;
}

let state: State = { appId: null, roles: [], users: [], assignments: [], availablePermissions: [], loading: false, error: null, isAdmin: false };
const listeners = new Set<() => void>();
let snapshot = state;

function emit() { snapshot = { ...state }; listeners.forEach((fn) => fn()); }
function errMsg(e: unknown) { return e instanceof Error ? e.message : String(e); }

export const subscribe = (fn: () => void) => (listeners.add(fn), () => listeners.delete(fn));
export const getSnapshot = () => snapshot;

async function api<T>(path: string, init?: RequestInit, fallback?: T): Promise<T> {
  const res = await fetchCore(path, init);
  if (!res.ok) {
    if (fallback !== undefined && res.status === 403) return fallback;
    throw new Error(await res.text().catch(() => res.statusText));
  }
  return res.json();
}

const json = (body: unknown): RequestInit => ({
  method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body),
});

function appPath(id: string) { return `/api/v1/apps/${encodeURIComponent(id)}`; }

async function fetchAll(appId: string) {
  const p = appPath(appId);
  const [roles, users, assignments, availablePermissions, perms] = await Promise.all([
    api<Role[]>(`${p}/roles`),
    api<User[]>("/api/v1/users"),
    api<Assignment[]>(`${p}/roles/assignments`, undefined, []),
    api<PermissionDeclaration[]>(`${p}/permissions/available`),
    api<{ permissions: string[] }>(`${p}/permissions`, undefined, { permissions: [] }),
  ]);
  return { roles, users, assignments, availablePermissions, isAdmin: perms.permissions.includes("*") };
}

export async function loadProject(projectPath: string) {
  state = { ...state, loading: true, error: null }; emit();
  try {
    const raw = await invoke<string>("read_file", { path: `${projectPath}/manifest.json` });
    const { appId } = JSON.parse(raw) as { appId: string };
    state = { ...state, appId, ...(await fetchAll(appId)), loading: false }; emit();
  } catch (e) { state = { ...state, loading: false, error: errMsg(e) }; emit(); }
}

export async function refresh() {
  if (!state.appId) return;
  state = { ...state, loading: true, error: null }; emit();
  try { state = { ...state, ...(await fetchAll(state.appId)), loading: false }; emit(); }
  catch (e) { state = { ...state, loading: false, error: errMsg(e) }; emit(); }
}

async function withRefresh(fn: () => Promise<unknown>) {
  try { await fn(); await refresh(); }
  catch (e) { state = { ...state, error: errMsg(e) }; emit(); }
}

export const assignRole = (userId: string, role: string) =>
  withRefresh(() => api(`${appPath(state.appId!)}/roles/assign`, json({ userId, role })));

export const revokeRole = (userId: string, role: string) =>
  withRefresh(() => api(`${appPath(state.appId!)}/roles/revoke`, json({ userId, role })));

export const createRole = (name: string, description?: string) =>
  withRefresh(() => api(`${appPath(state.appId!)}/roles`, json({ name, description })));

export const updateRole = (roleName: string, data: { description?: string; inherits?: string[]; permissions?: string[] }) =>
  withRefresh(() => api(`${appPath(state.appId!)}/roles/${encodeURIComponent(roleName)}`, { ...json(data), method: "PATCH" }));

export const deleteRole = (roleName: string) =>
  withRefresh(() => api(`${appPath(state.appId!)}/roles/${encodeURIComponent(roleName)}`, { method: "DELETE" }));
