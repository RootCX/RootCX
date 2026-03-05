import { invoke } from "@tauri-apps/api/core";
import { fetchCore } from "@/core/auth";

export interface Integration {
  id: string;
  name: string;
  version: string;
  description: string;
  actions: { id: string; name: string; description: string }[];
  configSchema: Record<string, unknown> | null;
  userAuth: { type: string; schema?: Record<string, unknown> } | null;
  webhooks: string[];
}

export interface Binding {
  integrationId: string;
  enabled: boolean;
  webhookToken: string | null;
  createdAt: string;
}

interface State {
  appId: string | null;
  catalog: Integration[];
  installed: Set<string>;
  bindings: Binding[];
  loading: boolean;
  error: string | null;
}

let state: State = { appId: null, catalog: [], installed: new Set(), bindings: [], loading: false, error: null };
const listeners = new Set<() => void>();
let snapshot = state;

function emit() { snapshot = { ...state }; listeners.forEach(fn => fn()); }
const err = (e: unknown) => e instanceof Error ? e.message : String(e);

async function api<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetchCore(path, init);
  if (!res.ok) throw new Error(await res.text().catch(() => "request failed"));
  return res.json();
}

export const subscribe = (fn: () => void) => (listeners.add(fn), () => listeners.delete(fn));
export const getSnapshot = () => snapshot;

export async function loadProject(projectPath: string) {
  state = { ...state, loading: true, error: null };
  emit();
  try {
    const raw = await invoke<string>("read_file", { path: `${projectPath}/manifest.json` });
    const { appId } = JSON.parse(raw) as { appId: string };
    state = { ...state, appId };
    emit();
    await refresh();
  } catch (e) {
    state = { ...state, loading: false, error: err(e) };
    emit();
  }
}

export async function refresh() {
  state = { ...state, loading: true, error: null };
  emit();
  try {
    const [catalog, installedList, bindings] = await Promise.all([
      api<Integration[]>("/api/v1/integrations/catalog"),
      api<Integration[]>("/api/v1/integrations"),
      state.appId
        ? api<Binding[]>(`/api/v1/apps/${encodeURIComponent(state.appId!)}/integrations`)
        : Promise.resolve([]),
    ]);
    state = { ...state, catalog, installed: new Set(installedList.map(i => i.id)), bindings, loading: false };
    emit();
  } catch (e) {
    state = { ...state, loading: false, error: err(e) };
    emit();
  }
}

export async function deploy(integrationId: string) {
  await api(`/api/v1/integrations/catalog/${encodeURIComponent(integrationId)}/deploy`, { method: "POST" });
  await refresh();
}

export async function undeploy(integrationId: string) {
  await api(`/api/v1/integrations/catalog/${encodeURIComponent(integrationId)}`, { method: "DELETE" });
  await refresh();
}

export async function bind(integrationId: string, config?: Record<string, string>) {
  if (!state.appId) return;
  await api(`/api/v1/apps/${encodeURIComponent(state.appId)}/integrations`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ integrationId, config }),
  });
  await refresh();
}

export async function updateConfig(integrationId: string, config: Record<string, string>) {
  if (!state.appId) return;
  await api(`/api/v1/apps/${encodeURIComponent(state.appId)}/integrations/${encodeURIComponent(integrationId)}`, {
    method: "PATCH",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ config }),
  });
  await refresh();
}

export async function unbind(integrationId: string) {
  if (!state.appId) return;
  await api(`/api/v1/apps/${encodeURIComponent(state.appId)}/integrations/${encodeURIComponent(integrationId)}`, {
    method: "DELETE",
  });
  await refresh();
}
