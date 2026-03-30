import { fetchCore } from "@/core/auth";

export interface Channel {
  id: string;
  provider: string;
  status: "active" | "inactive" | "error";
}

interface State { channels: Channel[]; loading: boolean; error: string | null }

let state: State = { channels: [], loading: false, error: null };
const listeners = new Set<() => void>();
let snapshot = state;

function emit() { snapshot = { ...state }; listeners.forEach(fn => fn()); }

async function api<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetchCore(path, init);
  if (!res.ok) {
    const text = await res.text().catch(() => "request failed");
    throw new Error(text);
  }
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
    state = { channels: await api<Channel[]>("/api/v1/channels"), loading: false, error: null }; emit();
  } catch (e) {
    state = { ...state, loading: false, error: e instanceof Error ? e.message : String(e) }; emit();
  }
}

export async function setup(provider: string, config: Record<string, string>) {
  const { id } = await api<{ id: string }>("/api/v1/channels", jsonPost({ provider, name: provider, config }));
  try {
    await api(`/api/v1/channels/${id}/activate`, { method: "POST" });
    await api(`/api/v1/channels/${id}/bindings`, jsonPost({ app_id: "assistant" }));
  } catch (e) {
    await api(`/api/v1/channels/${id}`, { method: "DELETE" }).catch(() => {});
    throw e;
  }
  await refresh();
}

export async function remove(id: string) {
  await api(`/api/v1/channels/${id}`, { method: "DELETE" });
  await refresh();
}
