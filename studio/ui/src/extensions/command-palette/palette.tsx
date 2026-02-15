import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { createPortal } from "react-dom";
import { commands, executeCommand, type Command } from "@/core/studio";
import type { Entry } from "@/core/registry";
import { getKeybindingForCommand } from "@/core/keybindings";
import { fuzzyMatch } from "@/lib/fuzzy";
import { subscribe, getIsOpen, closePalette } from "./store";

export function CommandPaletteOverlay() {
  const isOpen = useSyncExternalStore(subscribe, getIsOpen);
  if (!isOpen) return null;
  return createPortal(<PaletteDialog />, document.body);
}

interface ScoredEntry {
  entry: Entry<Command>;
  score: number;
  indices: number[];
}

function PaletteDialog() {
  const [query, setQuery] = useState("");
  const [selectedIndex, setSelectedIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);

  useEffect(() => { inputRef.current?.focus(); }, []);

  const allCommands = useSyncExternalStore(
    (cb) => commands.subscribe(cb),
    () => commands.getAll(),
  );

  const filtered = useMemo(() => {
    const visible = allCommands.filter((c) => c.id !== "commandPalette.open");
    if (!query) return visible.map((entry) => ({ entry, score: 0, indices: [] as number[] }));
    const results: ScoredEntry[] = [];
    for (const entry of visible) {
      const target = entry.category ? `${entry.category}: ${entry.title}` : entry.title;
      const m = fuzzyMatch(query, target);
      if (m) results.push({ entry, score: m.score, indices: m.indices });
    }
    return results.sort((a, b) => b.score - a.score);
  }, [allCommands, query]);

  const grouped = useMemo(() => {
    const groups = new Map<string, ScoredEntry[]>();
    for (const item of filtered) {
      const cat = item.entry.category ?? "Other";
      const list = groups.get(cat);
      if (list) list.push(item);
      else groups.set(cat, [item]);
    }
    return groups;
  }, [filtered]);

  useEffect(() => { setSelectedIndex(0); }, [query]);

  const execute = useCallback((entry: Entry<Command>) => {
    closePalette();
    executeCommand(entry.id);
  }, []);

  const handleKeyDown = useCallback((e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setSelectedIndex((i) => Math.min(i + 1, filtered.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setSelectedIndex((i) => Math.max(i - 1, 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      const item = filtered[selectedIndex];
      if (item) execute(item.entry);
    } else if (e.key === "Escape") {
      e.preventDefault();
      closePalette();
    }
  }, [filtered, selectedIndex, execute]);

  useEffect(() => {
    listRef.current?.querySelectorAll("button")[selectedIndex]?.scrollIntoView({ block: "nearest" });
  }, [selectedIndex]);

  let flatIndex = 0;

  return (
    <div className="fixed inset-0 z-50 flex items-start justify-center pt-[15vh]" onClick={closePalette}>
      <div className="absolute inset-0 bg-black/50" />
      <div
        className="relative w-full max-w-lg rounded-lg border border-border bg-card shadow-2xl"
        onClick={(e) => e.stopPropagation()}
        onKeyDown={handleKeyDown}
      >
        <div className="border-b border-border px-3 py-2">
          <input
            ref={inputRef}
            type="text"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Type a command..."
            className="w-full bg-transparent text-sm text-foreground placeholder:text-muted-foreground outline-none"
          />
        </div>
        <div ref={listRef} className="max-h-[300px] overflow-y-auto py-1">
          {filtered.length === 0 && (
            <div className="px-3 py-6 text-center text-sm text-muted-foreground">No matching commands</div>
          )}
          {[...grouped.entries()].map(([category, items]) => (
            <div key={category}>
              <div className="px-3 pt-2 pb-1 text-xs font-medium text-muted-foreground">{category}</div>
              {items.map((item) => {
                const idx = flatIndex++;
                const kb = getKeybindingForCommand(item.entry.id);
                return (
                  <button
                    key={item.entry.id}
                    className={`flex w-full items-center justify-between px-3 py-1.5 text-sm text-foreground cursor-pointer ${
                      idx === selectedIndex ? "bg-accent" : "hover:bg-accent/50"
                    }`}
                    onMouseEnter={() => setSelectedIndex(idx)}
                    onClick={() => execute(item.entry)}
                  >
                    <HighlightedText
                      text={item.entry.title}
                      indices={item.indices}
                      offset={item.entry.category ? item.entry.category.length + 2 : 0}
                    />
                    {kb && (
                      <kbd className="ml-4 shrink-0 rounded border border-border bg-muted px-1.5 py-0.5 text-xs text-muted-foreground">
                        {kb}
                      </kbd>
                    )}
                  </button>
                );
              })}
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

function HighlightedText({ text, indices, offset }: { text: string; indices: number[]; offset: number }) {
  if (indices.length === 0) return <>{text}</>;
  const adjusted = new Set(indices.filter((i) => i >= offset).map((i) => i - offset));
  if (adjusted.size === 0) return <>{text}</>;
  return (
    <>
      {[...text].map((ch, i) =>
        adjusted.has(i) ? (
          <span key={i} className="text-primary font-semibold">{ch}</span>
        ) : ch,
      )}
    </>
  );
}
