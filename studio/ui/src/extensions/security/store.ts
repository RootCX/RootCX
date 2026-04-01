import { fetchCore } from "@/core/auth";

export interface Role { name: string; description: string | null; inherits: string[]; permissions: string[] }
export interface User { id: string; email: string; displayName: string | null; createdAt: string }
export interface Assignment { userId: string; role: string; assignedAt: string }
export interface PermissionDeclaration { key: string; description: string }

interface State {
  roles: Role[]; users: User[]; assignments: Assignment[];
  availablePermissions: PermissionDeclaration[];
  loading: boolean; error: string | null; isAdmin: boolean;
}

let state: State = { roles: [], users: [], assignments: [], availablePermissions: [], loading: false, error: null, isAdmin: false };
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

async function fetchAll() {
  const [roles, users, assignments, availablePermissions, perms] = await Promise.all([
    api<Role[]>("/api/v1/roles"),
    api<User[]>("/api/v1/users"),
    api<Assignment[]>("/api/v1/roles/assignments", undefined, []),
    api<PermissionDeclaration[]>("/api/v1/permissions/available"),
    api<{ permissions: string[] }>("/api/v1/permissions", undefined, { permissions: [] }),
  ]);
  return { roles, users, assignments, availablePermissions, isAdmin: perms.permissions.includes("*") };
}

export async function refresh() {
  state = { ...state, loading: true, error: null }; emit();
  try { state = { ...state, ...(await fetchAll()), loading: false }; emit(); }
  catch (e) { state = { ...state, loading: false, error: errMsg(e) }; emit(); }
}

export const loadProject = refresh;

async function withRefresh(fn: () => Promise<unknown>) {
  try { await fn(); await refresh(); }
  catch (e) { state = { ...state, error: errMsg(e) }; emit(); }
}

export const assignRole = (userId: string, role: string) =>
  withRefresh(() => api("/api/v1/roles/assign", json({ userId, role })));

export const revokeRole = (userId: string, role: string) =>
  withRefresh(() => api("/api/v1/roles/revoke", json({ userId, role })));

export const createRole = (name: string, description?: string) =>
  withRefresh(() => api("/api/v1/roles", json({ name, description })));

export const updateRole = (roleName: string, data: { description?: string; inherits?: string[]; permissions?: string[] }) =>
  withRefresh(() => api(`/api/v1/roles/${encodeURIComponent(roleName)}`, { ...json(data), method: "PATCH" }));

export const deleteRole = (roleName: string) =>
  withRefresh(() => api(`/api/v1/roles/${encodeURIComponent(roleName)}`, { method: "DELETE" }));
