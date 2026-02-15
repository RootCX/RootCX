import { memo, useRef, useEffect, useCallback, useSyncExternalStore } from "react";
import { EditorView, keymap } from "@codemirror/view";
import { EditorState } from "@codemirror/state";
import type { LanguageSupport } from "@codemirror/language";
import { basicSetup } from "codemirror";
import { indentWithTab } from "@codemirror/commands";
import { X, Columns2, Rows2 } from "lucide-react";
import { studioTheme, studioHighlighting } from "./theme";
import { loadLanguage } from "./languages";
import {
  getSnapshot,
  setActiveTab,
  closeTab,
  splitPane,
  setFocusedPane,
  moveTab,
  updateContent,
  saveFile,
  isDirty,
  updateCursor,
  subscribeDirty,
  getDirtySnapshot,
  type PaneNode,
} from "./store";
import { cn } from "@/lib/utils";

// ── Drag state (shared across all pane instances) ──

let drag: {
  filePath: string;
  fromPaneId: string;
  startX: number;
  startY: number;
  active: boolean;
} | null = null;

const paneRefs = new Map<string, HTMLElement>();

function hitPane(x: number, y: number): string | null {
  for (const [id, el] of paneRefs) {
    const r = el.getBoundingClientRect();
    if (r.width > 0 && r.height > 0 && x >= r.left && x <= r.right && y >= r.top && y <= r.bottom)
      return id;
  }
  return null;
}

function clearHighlights() {
  for (const el of paneRefs.values()) el.classList.remove("drop-target");
}

// ── CodeMirror extensions ──

function makeExtensions(
  lang: LanguageSupport | null,
  paneIdRef: React.RefObject<string>,
  activeTabRef: React.RefObject<string | null>,
) {
  return [
    basicSetup,
    keymap.of([
      indentWithTab,
      { key: "Mod-s", run: () => { saveFile(paneIdRef.current!); return true; } },
    ]),
    studioTheme,
    studioHighlighting,
    ...(lang ? [lang] : []),
    EditorView.updateListener.of((update) => {
      if (update.docChanged && activeTabRef.current) {
        updateContent(activeTabRef.current, update.state.doc.toString());
      }
      if (update.selectionSet || update.docChanged) {
        const pos = update.state.selection.main.head;
        const line = update.state.doc.lineAt(pos);
        updateCursor(line.number, pos - line.from + 1);
      }
    }),
  ];
}

// ── Component ──

export const EditorPane = memo(function EditorPane({ pane, isFocused }: { pane: PaneNode; isFocused: boolean }) {
  useSyncExternalStore(subscribeDirty, getDirtySnapshot);
  const ref = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const statesRef = useRef<Map<string, EditorState>>(new Map());
  const activePathRef = useRef<string | null>(null);
  const paneIdRef = useRef(pane.id);

  // Register zone ref for drag hit-testing
  useEffect(() => {
    const el = ref.current;
    if (el) paneRefs.set(pane.id, el);
    return () => { paneRefs.delete(pane.id); };
  }, [pane.id]);

  // Destroy EditorView on unmount
  useEffect(() => {
    return () => {
      viewRef.current?.destroy();
      viewRef.current = null;
    };
  }, []);

  // Create EditorView once on mount (stable — no deps)
  const containerRef = useCallback((node: HTMLDivElement | null) => {
    if (!node || viewRef.current) return;
    viewRef.current = new EditorView({
      state: EditorState.create({ doc: "", extensions: makeExtensions(null, paneIdRef, activePathRef) }),
      parent: node,
    });
  }, []);

  const loadTab = useCallback(async (path: string) => {
    const view = viewRef.current;
    if (!view) return;

    if (activePathRef.current && activePathRef.current !== path) {
      statesRef.current.set(activePathRef.current, view.state);
    }
    activePathRef.current = path;

    const saved = statesRef.current.get(path);
    if (saved) { view.setState(saved); return; }

    const file = getSnapshot().files.get(path);
    if (!file) return;
    const lang = await loadLanguage(file.name);
    const newState = EditorState.create({ doc: file.content, extensions: makeExtensions(lang, paneIdRef, activePathRef) });
    statesRef.current.set(path, newState);
    view.setState(newState);
  }, []);

  useEffect(() => {
    if (pane.activeTab) loadTab(pane.activeTab);
  }, [pane.activeTab, loadTab]);

  // Clean stale cached states
  useEffect(() => {
    const paths = new Set(pane.tabs);
    for (const key of statesRef.current.keys()) {
      if (!paths.has(key)) statesRef.current.delete(key);
    }
  }, [pane.tabs]);

  const startDrag = useCallback((e: React.PointerEvent, filePath: string) => {
    e.preventDefault();
    setFocusedPane(pane.id);
    drag = { filePath, fromPaneId: pane.id, startX: e.clientX, startY: e.clientY, active: false };

    const cleanup = () => {
      document.removeEventListener("pointermove", onMove);
      document.removeEventListener("pointerup", onUp);
      document.removeEventListener("keydown", onKey);
      document.body.style.cursor = "";
      clearHighlights();
      drag = null;
    };

    const onMove = (ev: PointerEvent) => {
      if (!drag) return;
      if (!drag.active) {
        const dx = ev.clientX - drag.startX;
        const dy = ev.clientY - drag.startY;
        if (dx * dx + dy * dy < 25) return;
        drag.active = true;
        document.body.style.cursor = "grabbing";
      }
      const target = hitPane(ev.clientX, ev.clientY);
      for (const [id, el] of paneRefs) {
        el.classList.toggle("drop-target", id === target && target !== drag.fromPaneId);
      }
    };

    const onUp = (ev: PointerEvent) => {
      const wasDrag = drag?.active;
      const d = drag;
      cleanup();
      if (!wasDrag || !d) {
        setActiveTab(filePath, pane.id);
        return;
      }
      const target = hitPane(ev.clientX, ev.clientY);
      if (target && target !== d.fromPaneId) {
        moveTab(d.filePath, d.fromPaneId, target);
      }
    };

    const onKey = (ev: KeyboardEvent) => {
      if (ev.key === "Escape") cleanup();
    };

    document.addEventListener("pointermove", onMove);
    document.addEventListener("pointerup", onUp);
    document.addEventListener("keydown", onKey);
  }, [pane.id]);

  const { files } = getSnapshot();
  const hasTabs = pane.tabs.length > 0;

  return (
    <div
      ref={ref}
      className={cn("flex h-full flex-col", isFocused && "ring-1 ring-primary/30")}
      onPointerDown={() => setFocusedPane(pane.id)}
    >
      {!hasTabs && (
        <div className="flex flex-1 items-center justify-center text-sm text-muted-foreground">
          Open a file from the Explorer
        </div>
      )}

      {hasTabs && (
        <>
          <div className="flex shrink-0 border-b border-border bg-panel">
            <div className="flex flex-1 overflow-x-auto">
              {pane.tabs.map((path) => {
                const file = files.get(path);
                if (!file) return null;
                return (
                  <button
                    key={path}
                    onPointerDown={(e) => startDrag(e, path)}
                    className={cn(
                      "group flex items-center gap-1.5 border-r border-border px-3 py-1.5 text-xs",
                      path === pane.activeTab
                        ? "bg-background text-foreground"
                        : "text-muted-foreground hover:bg-accent",
                    )}
                  >
                    <span className="max-w-[120px] truncate">{file.name}</span>
                    {isDirty(path) && <span className="h-2 w-2 shrink-0 rounded-full bg-primary" />}
                    <span
                      role="button"
                      onClick={(e) => { e.stopPropagation(); closeTab(path, pane.id); }}
                      className="ml-1 shrink-0 rounded p-0.5 opacity-0 hover:bg-muted group-hover:opacity-100"
                    >
                      <X className="h-3 w-3" />
                    </span>
                  </button>
                );
              })}
            </div>
            <div className="flex shrink-0 items-center gap-0.5 px-1">
              <button
                onClick={() => splitPane(pane.id, "horizontal")}
                className="flex h-5 w-5 items-center justify-center rounded text-muted-foreground hover:bg-accent hover:text-foreground"
                title="Split Right"
              >
                <Columns2 className="h-3.5 w-3.5" />
              </button>
              <button
                onClick={() => splitPane(pane.id, "vertical")}
                className="flex h-5 w-5 items-center justify-center rounded text-muted-foreground hover:bg-accent hover:text-foreground"
                title="Split Down"
              >
                <Rows2 className="h-3.5 w-3.5" />
              </button>
            </div>
          </div>
          <div ref={containerRef} className="flex-1 overflow-hidden" />
        </>
      )}
    </div>
  );
});
