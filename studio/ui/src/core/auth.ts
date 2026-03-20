import { useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";

let BASE = "";

export async function setCoreUrl(url: string) {
  BASE = url.replace(/\/+$/, "");
  await invoke("set_core_url", { url: BASE });
}

export async function getCoreUrl(): Promise<string> {
  BASE = await invoke<string>("get_core_url");
  return BASE;
}

const REFRESH_KEY = "rootcx_refresh_token";
const CONNECTIONS_KEY = "rootcx_connections";

export interface AuthUser {
  id: string;
  username: string;
  email: string | null;
  displayName: string | null;
}

interface AuthState {
  user: AuthUser | null;
  loading: boolean;
  connected: boolean;
}

let state: AuthState = { user: null, loading: true, connected: false };
let accessToken: string | null = null;
let refreshToken: string | null = null;
const listeners = new Set<() => void>();
let snapshot = state;

function emit() { snapshot = { ...state }; listeners.forEach((fn) => fn()); }
function syncToken() { invoke("set_auth_token", { token: accessToken ?? "" }).catch(() => {}); }
function clearTokens() { accessToken = null; refreshToken = null; localStorage.removeItem(REFRESH_KEY); }

const subscribe = (fn: () => void) => (listeners.add(fn), () => listeners.delete(fn));
const getSnapshot = () => snapshot;
export function useAuth() { return useSyncExternalStore(subscribe, getSnapshot); }

async function checkHealth(): Promise<boolean> {
  try {
    const res = await fetch(`${BASE}/health`, { signal: AbortSignal.timeout(3000) });
    return res.ok;
  } catch { return false; }
}

async function tryRestoreSession() {
  const stored = localStorage.getItem(REFRESH_KEY);
  if (!stored) return;
  refreshToken = stored;
  try {
    await doRefresh();
    state = { ...state, user: await fetchMe() };
    emit();
  } catch { clearTokens(); }
}

export async function initAuth() {
  try {
    const url = await getCoreUrl();
    if (url && await checkHealth()) {
      state = { ...state, connected: true };
      await tryRestoreSession();
    }
    state = { ...state, loading: false };
    emit();
  } catch {
    state = { user: null, loading: false, connected: false };
    emit();
  }
}

export async function connectTo(url: string): Promise<boolean> {
  const clean = url.replace(/\/+$/, "");
  if (!/^https?:\/\//i.test(clean)) return false;
  const prev = BASE;
  BASE = clean;
  if (!(await checkHealth())) { BASE = prev; return false; }
  await setCoreUrl(clean);
  pushConnection(clean);
  state = { ...state, connected: true };
  emit();
  await tryRestoreSession();
  return true;
}

export async function disconnect() {
  clearTokens();
  syncToken();
  BASE = "";
  await setCoreUrl("");
  state = { user: null, loading: false, connected: false };
  emit();
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

export function getSavedConnections(): { url: string; lastUsed: number }[] {
  try { return JSON.parse(localStorage.getItem(CONNECTIONS_KEY) ?? "[]"); }
  catch { return []; }
}

function pushConnection(url: string) {
  const conns = getSavedConnections().filter((c) => c.url !== url);
  conns.unshift({ url, lastUsed: Date.now() });
  if (conns.length > 10) conns.length = 10;
  localStorage.setItem(CONNECTIONS_KEY, JSON.stringify(conns));
}
