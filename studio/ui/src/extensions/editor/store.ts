import { invoke } from "@tauri-apps/api/core";

// ── Types ──

export interface FileData {
  path: string;
  name: string;
  content: string;
  savedContent: string;
}

export interface PaneNode {
  type: "pane";
  id: string;
  tabs: string[];
  activeTab: string | null;
}

export interface SplitNode {
  type: "split";
  id: string;
  direction: "horizontal" | "vertical";
  children: EditorNode[];
}

export type EditorNode = PaneNode | SplitNode;

interface EditorState {
  files: Map<string, FileData>;
  root: EditorNode;
  focusedPane: string;
}

type Listener = () => void;

function createSignal<T>(initial: T) {
  let value = initial;
  const listeners = new Set<Listener>();
  return {
    get: () => value,
    set: (next: T) => { value = next; listeners.forEach((fn) => fn()); },
    subscribe: (fn: Listener) => { listeners.add(fn); return () => listeners.delete(fn); },
  };
}

// ── Helpers ──

let nodeCounter = 0;
function nextId(prefix: string) { return `${prefix}-${++nodeCounter}`; }

function makePane(tabs: string[] = [], activeTab: string | null = null): PaneNode {
  return { type: "pane", id: nextId("p"), tabs, activeTab };
}

function makeSplit(direction: "horizontal" | "vertical", children: EditorNode[]): SplitNode {
  return { type: "split", id: nextId("s"), direction, children };
}

export function findPane(node: EditorNode, id: string): PaneNode | null {
  if (node.type === "pane") return node.id === id ? node : null;
  for (const child of node.children) {
    const found = findPane(child, id);
    if (found) return found;
  }
  return null;
}

function replaceNode(root: EditorNode, targetId: string, replacement: EditorNode): EditorNode {
  if (root.type === "pane") return root.id === targetId ? replacement : root;
  if (root.id === targetId) return replacement;
  let changed = false;
  const children = root.children.map((c) => {
    const next = replaceNode(c, targetId, replacement);
    if (next !== c) changed = true;
    return next;
  });
  return changed ? { ...root, children } : root;
}

function removePane(root: EditorNode, paneId: string): EditorNode | null {
  if (root.type === "pane") return root.id === paneId ? null : root;
  let changed = false;
  const children: EditorNode[] = [];
  for (const c of root.children) {
    const next = removePane(c, paneId);
    if (next !== c) changed = true;
    if (next) children.push(next);
  }
  if (!changed) return root;
  if (children.length === 0) return null;
  if (children.length === 1) return children[0];
  return { ...root, children };
}

function firstPane(node: EditorNode): PaneNode | null {
  if (node.type === "pane") return node;
  for (const child of node.children) {
    const found = firstPane(child);
    if (found) return found;
  }
  return null;
}

function collectOpenPaths(node: EditorNode, out = new Set<string>()): Set<string> {
  if (node.type === "pane") { for (const t of node.tabs) out.add(t); }
  else { for (const c of node.children) collectOpenPaths(c, out); }
  return out;
}

// ── State ──

const initialPane = makePane();

let state: EditorState = {
  files: new Map(),
  root: initialPane,
  focusedPane: initialPane.id,
};

const main = createSignal(state);
function emit() { main.set({ ...state }); }
export const subscribe = main.subscribe;
export const getSnapshot = main.get;

// ── Actions ──

export async function openFile(path: string, paneId?: string) {
  const targetId = paneId ?? state.focusedPane;
  const pane = findPane(state.root, targetId);
  if (!pane) return;

  if (!state.files.has(path)) {
    const content = await invoke<string>("read_file", { path });
    const name = path.split("/").pop() ?? path;
    state = { ...state, files: new Map(state.files).set(path, { path, name, content, savedContent: content }) };
  }

  const tabs = pane.tabs.includes(path) ? pane.tabs : [...pane.tabs, path];
  state = { ...state, root: replaceNode(state.root, pane.id, { ...pane, tabs, activeTab: path }) };
  emit();
}

export function closeTab(path: string, paneId?: string) {
  const targetId = paneId ?? state.focusedPane;
  const pane = findPane(state.root, targetId);
  if (!pane) return;

  const idx = pane.tabs.indexOf(path);
  if (idx === -1) return;

  const tabs = pane.tabs.filter((t) => t !== path);
  let activeTab = pane.activeTab;
  if (activeTab === path) {
    activeTab = tabs[Math.min(idx, tabs.length - 1)] ?? null;
  }

  if (tabs.length === 0) {
    const pruned = removePane(state.root, pane.id);
    if (!pruned) {
      state = { ...state, root: replaceNode(state.root, pane.id, { ...pane, tabs: [], activeTab: null }) };
    } else {
      const newFocus = firstPane(pruned)?.id ?? state.focusedPane;
      state = { ...state, root: pruned, focusedPane: newFocus };
    }
  } else {
    state = { ...state, root: replaceNode(state.root, pane.id, { ...pane, tabs, activeTab }) };
  }

  // GC files not open in any pane (defer clone until needed)
  const open = collectOpenPaths(state.root);
  let files: Map<string, FileData> | undefined;
  for (const key of state.files.keys()) {
    if (!open.has(key)) (files ??= new Map(state.files)).delete(key);
  }
  if (files) state = { ...state, files };
  emit();
}

export function setActiveTab(path: string, paneId: string) {
  const pane = findPane(state.root, paneId);
  if (!pane || !pane.tabs.includes(path) || pane.activeTab === path) return;
  state = { ...state, root: replaceNode(state.root, pane.id, { ...pane, activeTab: path }) };
  emit();
}

export function setFocusedPane(paneId: string) {
  if (state.focusedPane === paneId) return;
  state = { ...state, focusedPane: paneId };
  emit();
}

export function splitPane(paneId: string, direction: "horizontal" | "vertical") {
  const pane = findPane(state.root, paneId);
  if (!pane || !pane.activeTab) return;

  const filePath = pane.activeTab;
  const newPane = makePane([filePath], filePath);

  const oldTabs = pane.tabs.filter((t) => t !== filePath);
  const updatedOld: PaneNode = {
    ...pane,
    tabs: oldTabs.length > 0 ? oldTabs : [filePath],
    activeTab: oldTabs[0] ?? filePath,
  };

  const split = makeSplit(direction, [updatedOld, newPane]);
  state = {
    ...state,
    root: replaceNode(state.root, pane.id, split),
    focusedPane: newPane.id,
  };
  emit();
}

export function moveTab(path: string, fromPaneId: string, toPaneId: string, index?: number) {
  if (fromPaneId === toPaneId) return;
  const fromPane = findPane(state.root, fromPaneId);
  const toPane = findPane(state.root, toPaneId);
  if (!fromPane || !toPane) return;

  const fromTabs = fromPane.tabs.filter((t) => t !== path);
  const fromActive = fromPane.activeTab === path
    ? (fromTabs[Math.min(fromPane.tabs.indexOf(path), fromTabs.length - 1)] ?? null)
    : fromPane.activeTab;

  const toTabs = toPane.tabs.includes(path) ? toPane.tabs : [...toPane.tabs];
  if (!toPane.tabs.includes(path)) {
    toTabs.splice(index ?? toTabs.length, 0, path);
  }

  let root = state.root;
  root = replaceNode(root, toPane.id, { ...toPane, tabs: toTabs, activeTab: path });

  if (fromTabs.length === 0) {
    root = removePane(root, fromPane.id) ?? makePane();
  } else {
    root = replaceNode(root, fromPane.id, { ...fromPane, tabs: fromTabs, activeTab: fromActive });
  }

  state = { ...state, root, focusedPane: toPaneId };
  emit();
}

export function updateContent(path: string, content: string) {
  const file = state.files.get(path);
  if (!file || file.content === content) return;
  const wasDirty = file.content !== file.savedContent;
  file.content = content;
  const nowDirty = file.content !== file.savedContent;
  if (wasDirty !== nowDirty) emitDirty();
}

export async function saveFile(paneId?: string) {
  const targetId = paneId ?? state.focusedPane;
  const pane = findPane(state.root, targetId);
  if (!pane?.activeTab) return;
  const file = state.files.get(pane.activeTab);
  if (!file) return;
  await invoke("write_file", { path: file.path, contents: file.content });
  file.savedContent = file.content;
  emitDirty();
  emit();
}

export function isDirty(path: string): boolean {
  const file = state.files.get(path);
  return file ? file.content !== file.savedContent : false;
}

export function getFocusedFile(): FileData | null {
  const pane = findPane(state.root, state.focusedPane);
  if (!pane?.activeTab) return null;
  return state.files.get(pane.activeTab) ?? null;
}

// ── Dirty tracking (lightweight, separate from main store) ──

const dirty = createSignal(0);
export const subscribeDirty = dirty.subscribe;
export const getDirtySnapshot = dirty.get;
function emitDirty() { dirty.set(dirty.get() + 1); }

// ── Cursor (separate subscription) ──

const cursor = createSignal("Ln 1, Col 1");
export const subscribeCursor = cursor.subscribe;
export const getCursorSnapshot = cursor.get;

export function updateCursor(line: number, col: number) {
  const next = `Ln ${line}, Col ${col}`;
  if (next !== cursor.get()) cursor.set(next);
}
