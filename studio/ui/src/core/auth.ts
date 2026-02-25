import { useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";

const BASE = "http://localhost:9100";
const REFRESH_KEY = "rootcx_refresh_token";

export interface AuthUser {
  id: string;
  username: string;
  email: string | null;
  displayName: string | null;
}

interface AuthState {
  user: AuthUser | null;
  loading: boolean;
  authRequired: boolean;
}

let state: AuthState = { user: null, loading: true, authRequired: false };
let accessToken: string | null = null;
let refreshToken: string | null = null;
const listeners = new Set<() => void>();
let snapshot = state;

function emit() { snapshot = { ...state }; listeners.forEach((fn) => fn()); }
function syncToken() { invoke("set_auth_token", { token: accessToken ?? "" }).catch(() => {}); }
function clearTokens() { accessToken = null; refreshToken = null; localStorage.removeItem(REFRESH_KEY); }

export const subscribe = (fn: () => void) => (listeners.add(fn), () => listeners.delete(fn));
export const getSnapshot = () => snapshot;
export function useAuth() { return useSyncExternalStore(subscribe, getSnapshot); }

const ANON: AuthUser = { id: "anonymous", username: "anonymous", email: null, displayName: null };

export async function initAuth() {
  try {
    const res = await fetch(`${BASE}/api/v1/auth/mode`);
    if (!res.ok) { state = { ...state, loading: false }; emit(); return; }

    const { authRequired } = await res.json();
    state = { ...state, authRequired };

    if (!authRequired) { state = { ...state, user: ANON, loading: false }; emit(); return; }

    const stored = localStorage.getItem(REFRESH_KEY);
    if (stored) {
      refreshToken = stored;
      try {
        await doRefresh();
        state = { ...state, user: await fetchMe(), loading: false };
        emit();
        return;
      } catch { clearTokens(); }
    }

    state = { ...state, loading: false };
    emit();
  } catch {
    state = { ...state, loading: false };
    emit();
  }
}

export async function login(username: string, password: string) {
  const res = await fetch(`${BASE}/api/v1/auth/login`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ username, password }),
  });
  if (!res.ok) throw new Error(await res.text().catch(() => "login failed"));
  const data = await res.json();
  accessToken = data.accessToken;
  refreshToken = data.refreshToken;
  if (refreshToken) localStorage.setItem(REFRESH_KEY, refreshToken);
  syncToken();
  state = { ...state, user: data.user };
  emit();
}

export async function register(input: { username: string; password: string; email?: string; displayName?: string }) {
  const res = await fetch(`${BASE}/api/v1/auth/register`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(input),
  });
  if (!res.ok) throw new Error(await res.text().catch(() => "registration failed"));
  await login(input.username, input.password);
}

export async function logout() {
  if (refreshToken) {
    fetch(`${BASE}/api/v1/auth/logout`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ refreshToken }),
    }).catch(() => {});
  }
  clearTokens();
  syncToken();
  state = { ...state, user: null };
  emit();
}

async function doRefresh() {
  if (!refreshToken) throw new Error("no refresh token");
  const res = await fetch(`${BASE}/api/v1/auth/refresh`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ refreshToken }),
  });
  if (!res.ok) { clearTokens(); throw new Error("refresh failed"); }
  accessToken = (await res.json()).accessToken;
  syncToken();
}

async function fetchMe(): Promise<AuthUser> {
  const res = await fetch(`${BASE}/api/v1/auth/me`, { headers: { Authorization: `Bearer ${accessToken}` } });
  if (!res.ok) throw new Error("failed to fetch user");
  return res.json();
}

export function fetchCore(path: string, init?: RequestInit): Promise<Response> {
  const doFetch = (token: string | null) => {
    const headers = new Headers(init?.headers);
    if (token) headers.set("Authorization", `Bearer ${token}`);
    return fetch(`${BASE}${path}`, { ...init, headers });
  };
  return doFetch(accessToken).then((res) => {
    if (res.status !== 401 || !refreshToken) return res;
    return doRefresh().then(() => doFetch(accessToken), () => { logout(); return res; });
  });
}
