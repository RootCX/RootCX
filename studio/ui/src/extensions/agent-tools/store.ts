import { invoke } from "@tauri-apps/api/core";
import { notify } from "@/core/notifications";
import { executeCommand } from "@/core/studio";
import { fetchCore } from "@/core/auth";

const IMPLICIT_TOOLS = new Set(["query_data", "mutate_data"]);

export interface ToolEntry {
  name: string;
  description: string;
  enabled: boolean;
  implicit: boolean;
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
  agent?: {
    access?: Array<{ entity: string; actions?: string[] }>;
    [key: string]: unknown;
  };
  [key: string]: unknown;
}

async function readManifest(projectPath: string): Promise<Manifest> {
  const raw = await invoke<string>("read_file", { path: `${projectPath}/manifest.json` });
  return JSON.parse(raw);
}

async function writeManifest(projectPath: string, manifest: Manifest) {
  await invoke("write_file", {
    path: `${projectPath}/manifest.json`,
    contents: JSON.stringify(manifest, null, 2) + "\n",
  });
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

function buildToolEntries(
  available: ToolInfo[],
  access: Array<{ entity: string; actions?: string[] }>,
  hasEntities: boolean,
): ToolEntry[] {
  const enabledTools = new Set(
    access
      .filter((a) => a.entity.startsWith("tool:"))
      .map((a) => a.entity.slice(5)),
  );

  return available.map((t) => {
    const implicit = IMPLICIT_TOOLS.has(t.name);
    return {
      name: t.name,
      description: t.description,
      enabled: implicit ? hasEntities : enabledTools.has(t.name),
      implicit,
    };
  });
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
    const access = manifest.agent.access ?? [];
    const hasEntities = access.some((a) => !a.entity.startsWith("tool:"));
    const available = await fetchAvailableTools();
    state = {
      ...state,
      isAgent: true,
      tools: buildToolEntries(available, access, hasEntities),
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

export async function toggleTool(name: string) {
  if (!state.projectPath) return;
  const entry = state.tools.find((t) => t.name === name);
  if (!entry || entry.implicit) return;

  const manifest = await readManifest(state.projectPath);
  if (!manifest.agent) return;

  const access = manifest.agent.access ?? [];
  const toolEntity = `tool:${name}`;
  const idx = access.findIndex((a) => a.entity === toolEntity);

  if (idx >= 0) {
    access.splice(idx, 1);
  } else {
    access.push({ entity: toolEntity, actions: [] });
  }

  manifest.agent.access = access;
  await writeManifest(state.projectPath, manifest);
  notify("agent-tools-changed", "Tool access changed — re-run to apply", "warning", {
    label: "Run",
    run: () => executeCommand("rootcx.run"),
  });

  state = {
    ...state,
    tools: state.tools.map((t) =>
      t.name === name ? { ...t, enabled: !t.enabled } : t,
    ),
  };
  emit();
}
