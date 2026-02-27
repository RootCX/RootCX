import { invoke } from "@tauri-apps/api/core";
import { fetchCore } from "@/core/auth";

export interface ToolEntry {
  name: string;
  description: string;
}

interface State {
  tools: ToolEntry[];
  isAgent: boolean;
  loading: boolean;
  projectPath: string | null;
  appId: string | null;
  agentName: string | null;
}

let state: State = { tools: [], isAgent: false, loading: false, projectPath: null, appId: null, agentName: null };
const listeners = new Set<() => void>();
let snapshot = state;

function emit() {
  snapshot = { ...state };
  listeners.forEach((fn) => fn());
}

export const subscribe = (fn: () => void) => (listeners.add(fn), () => listeners.delete(fn));
export const getSnapshot = () => snapshot;

interface Manifest {
  appId: string;
  name?: string;
  agent?: Record<string, unknown>;
  [key: string]: unknown;
}

async function readManifest(projectPath: string): Promise<Manifest> {
  const raw = await invoke<string>("read_file", { path: `${projectPath}/manifest.json` });
  return JSON.parse(raw);
}

interface ToolInfo {
  name: string;
  description: string;
}

async function fetchAvailableTools(): Promise<ToolInfo[]> {
  const res = await fetchCore("/api/v1/tools");
  if (!res.ok) throw new Error("failed to fetch tools");
  return res.json();
}

export async function loadProject(projectPath: string) {
  state = { ...state, loading: true, projectPath };
  emit();
  try {
    const manifest = await readManifest(projectPath);
    if (!manifest.agent) {
      state = { ...state, isAgent: false, tools: [], loading: false, appId: null, agentName: null };
      emit();
      return;
    }
    const available = await fetchAvailableTools();
    state = {
      ...state,
      isAgent: true,
      tools: available.map((t) => ({ name: t.name, description: t.description })),
      loading: false,
      appId: manifest.appId,
      agentName: manifest.name ?? manifest.appId,
    };
    emit();
  } catch {
    state = { ...state, loading: false };
    emit();
  }
}
