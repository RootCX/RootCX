import { fetchCore } from "@/core/auth";

export interface McpServer {
  name: string;
  config: McpServerConfig;
  status: "running" | "stopped" | "error";
}

export interface McpServerConfig {
  name: string;
  description?: string;
  transport: { type: "stdio"; command: string; args?: string[] } | { type: "sse"; url: string; headers?: Record<string, string> };
}

interface State { servers: McpServer[]; loading: boolean; error: string | null }

let state: State = { servers: [], loading: false, error: null };
const listeners = new Set<() => void>();
let snapshot = state;

function emit() { snapshot = { ...state }; listeners.forEach(fn => fn()); }

async function api<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetchCore(path, init);
  if (!res.ok) throw new Error(await res.text().catch(() => "request failed"));
  return res.json();
}

const jsonPost = (body: unknown): RequestInit => ({
  method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body),
});

export const subscribe = (fn: () => void) => (listeners.add(fn), () => listeners.delete(fn));
export const getSnapshot = () => snapshot;

export async function refresh() {
  state = { ...state, loading: true, error: null }; emit();
  try {
    state = { servers: await api<McpServer[]>("/api/v1/mcp-servers"), loading: false, error: null }; emit();
  } catch (e) {
    state = { ...state, loading: false, error: e instanceof Error ? e.message : String(e) }; emit();
  }
}

export async function register(config: McpServerConfig, autoStart: boolean) {
  await api("/api/v1/mcp-servers", jsonPost({ ...config, autoStart }));
  await refresh();
}

export async function remove(name: string) {
  await api(`/api/v1/mcp-servers/${encodeURIComponent(name)}`, { method: "DELETE" });
  await refresh();
}

export async function start(name: string) {
  await api(`/api/v1/mcp-servers/${encodeURIComponent(name)}/start`, { method: "POST" });
  await refresh();
}

export async function stop(name: string) {
  await api(`/api/v1/mcp-servers/${encodeURIComponent(name)}/stop`, { method: "POST" });
  await refresh();
}
