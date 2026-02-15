import { invoke } from "@tauri-apps/api/core";

export interface EditorTab {
  path: string;
  name: string;
  content: string;
  savedContent: string;
}

interface EditorState {
  tabs: EditorTab[];
  activeTab: string | null;
}

type Listener = () => void;

let state: EditorState = { tabs: [], activeTab: null };
const listeners = new Set<Listener>();
let snapshot = state;

function emit() {
  snapshot = { ...state };
  listeners.forEach((fn) => fn());
}

export function subscribe(fn: Listener): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

export function getSnapshot(): EditorState {
  return snapshot;
}

export async function openFile(path: string) {
  const existing = state.tabs.find((t) => t.path === path);
  if (existing) {
    state = { ...state, activeTab: path };
    emit();
    return;
  }

  const content = await invoke<string>("read_file", { path });
  const name = path.split("/").pop() ?? path;
  state = {
    tabs: [...state.tabs, { path, name, content, savedContent: content }],
    activeTab: path,
  };
  emit();
}

export function closeTab(path: string) {
  const idx = state.tabs.findIndex((t) => t.path === path);
  if (idx === -1) return;

  const tabs = state.tabs.filter((t) => t.path !== path);
  let activeTab = state.activeTab;
  if (activeTab === path) {
    activeTab = tabs[Math.min(idx, tabs.length - 1)]?.path ?? null;
  }
  state = { tabs, activeTab };
  emit();
}

export function setActiveTab(path: string) {
  state = { ...state, activeTab: path };
  emit();
}

export function updateContent(path: string, content: string) {
  state = {
    ...state,
    tabs: state.tabs.map((t) => (t.path === path ? { ...t, content } : t)),
  };
  emit();
}

export async function saveFile() {
  const tab = state.tabs.find((t) => t.path === state.activeTab);
  if (!tab) return;
  await invoke("write_file", { path: tab.path, contents: tab.content });
  state = {
    ...state,
    tabs: state.tabs.map((t) =>
      t.path === tab.path ? { ...t, savedContent: t.content } : t,
    ),
  };
  emit();
}

export function isDirty(path: string): boolean {
  const tab = state.tabs.find((t) => t.path === path);
  return tab ? tab.content !== tab.savedContent : false;
}

// ── Cursor position (separate subscription to avoid re-rendering tabs on every keystroke) ──

let cursorSnapshot = "Ln 1, Col 1";
const cursorListeners = new Set<Listener>();

export function subscribeCursor(fn: Listener): () => void {
  cursorListeners.add(fn);
  return () => cursorListeners.delete(fn);
}

export function getCursorSnapshot() {
  return cursorSnapshot;
}

export function updateCursor(line: number, col: number) {
  const next = `Ln ${line}, Col ${col}`;
  if (next === cursorSnapshot) return;
  cursorSnapshot = next;
  cursorListeners.forEach((fn) => fn());
}
