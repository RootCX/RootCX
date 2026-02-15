import { useRef, useEffect, useSyncExternalStore, useCallback } from "react";
import { EditorView, keymap } from "@codemirror/view";
import { EditorState } from "@codemirror/state";
import type { LanguageSupport } from "@codemirror/language";
import { basicSetup } from "codemirror";
import { indentWithTab } from "@codemirror/commands";
import { studioTheme, studioHighlighting } from "./theme";
import { loadLanguage } from "./languages";
import {
  subscribe,
  getSnapshot,
  setActiveTab,
  closeTab,
  updateContent,
  saveFile,
  isDirty,
  updateCursor,
} from "./store";
import { cn } from "@/lib/utils";
import { X } from "lucide-react";

function makeExtensions(lang: LanguageSupport | null) {
  return [
    basicSetup,
    keymap.of([
      indentWithTab,
      { key: "Mod-s", run: () => { saveFile(); return true; } },
    ]),
    studioTheme,
    studioHighlighting,
    ...(lang ? [lang] : []),
    EditorView.updateListener.of((update) => {
      if (update.docChanged) {
        const active = getSnapshot().activeTab;
        if (active) updateContent(active, update.state.doc.toString());
      }
      if (update.selectionSet || update.docChanged) {
        const pos = update.state.selection.main.head;
        const line = update.state.doc.lineAt(pos);
        updateCursor(line.number, pos - line.from + 1);
      }
    }),
  ];
}

export default function EditorPanel() {
  const { tabs, activeTab } = useSyncExternalStore(subscribe, getSnapshot);
  const viewRef = useRef<EditorView | null>(null);
  const statesRef = useRef<Map<string, EditorState>>(new Map());
  const activePathRef = useRef<string | null>(null);

  const containerRef = useCallback((node: HTMLDivElement | null) => {
    if (!node || viewRef.current) return;
    viewRef.current = new EditorView({
      state: EditorState.create({ doc: "", extensions: makeExtensions(null) }),
      parent: node,
    });
  }, []);

  const loadTab = useCallback(async (path: string, content: string, name: string) => {
    const view = viewRef.current;
    if (!view) return;

    if (activePathRef.current && activePathRef.current !== path) {
      statesRef.current.set(activePathRef.current, view.state);
    }
    activePathRef.current = path;

    const saved = statesRef.current.get(path);
    if (saved) {
      view.setState(saved);
      return;
    }

    const lang = await loadLanguage(name);
    const newState = EditorState.create({ doc: content, extensions: makeExtensions(lang) });
    statesRef.current.set(path, newState);
    view.setState(newState);
  }, []);

  useEffect(() => {
    if (!activeTab) return;
    const tab = getSnapshot().tabs.find((t) => t.path === activeTab);
    if (tab) loadTab(tab.path, tab.content, tab.name);
  }, [activeTab, loadTab]);

  useEffect(() => {
    const paths = new Set(tabs.map((t) => t.path));
    for (const key of statesRef.current.keys()) {
      if (!paths.has(key)) statesRef.current.delete(key);
    }
  }, [tabs]);

  const hasTabs = tabs.length > 0;

  return (
    <div className="relative flex h-full flex-col">
      {!hasTabs && (
        <div className="absolute inset-0 flex items-center justify-center text-sm text-muted-foreground">
          Open a file from the Explorer
        </div>
      )}

      {hasTabs && (
        <div className="flex shrink-0 overflow-x-auto border-b border-border bg-panel">
          {tabs.map((tab) => (
            <button
              key={tab.path}
              onClick={() => setActiveTab(tab.path)}
              className={cn(
                "group flex items-center gap-1.5 border-r border-border px-3 py-1.5 text-xs",
                tab.path === activeTab
                  ? "bg-background text-foreground"
                  : "text-muted-foreground hover:bg-accent",
              )}
            >
              <span className="max-w-[120px] truncate">{tab.name}</span>
              {isDirty(tab.path) && (
                <span className="h-2 w-2 shrink-0 rounded-full bg-primary" />
              )}
              <span
                role="button"
                onClick={(e) => { e.stopPropagation(); closeTab(tab.path); }}
                className="ml-1 shrink-0 rounded p-0.5 opacity-0 hover:bg-muted group-hover:opacity-100"
              >
                <X className="h-3 w-3" />
              </span>
            </button>
          ))}
        </div>
      )}

      <div ref={containerRef} className={cn("flex-1 overflow-hidden", !hasTabs && "invisible")} />
    </div>
  );
}
