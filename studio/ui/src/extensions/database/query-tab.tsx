import { useState, useSyncExternalStore } from "react";
import { Play } from "lucide-react";
import { Button } from "@/components/ui/button";
import { executeQuery, subscribe, getSnapshot } from "./store";

function formatValue(val: unknown): string {
  if (val === null || val === undefined) return "NULL";
  if (typeof val === "object") return JSON.stringify(val);
  return String(val);
}

function cellClass(val: unknown): string {
  if (val === null || val === undefined) return "text-muted-foreground/40 italic";
  if (typeof val === "number") return "text-right tabular-nums";
  if (typeof val === "boolean") return "text-blue-400";
  return "";
}

function Elapsed({ ms }: { ms: number | null }) {
  if (ms === null) return null;
  return <span className="text-[10px] text-muted-foreground/50">{ms.toFixed(0)}ms</span>;
}

function ResultsGrid() {
  const { queryResult, queryError, queryLoading, queryElapsed } = useSyncExternalStore(subscribe, getSnapshot);

  if (queryLoading)
    return <div className="flex items-center justify-center py-8 text-xs text-muted-foreground animate-pulse">Executing...</div>;

  if (queryError)
    return (
      <div className="p-3">
        <pre className="whitespace-pre-wrap rounded border border-red-900/30 bg-red-950/20 p-3 text-xs text-red-300">{queryError}</pre>
        <div className="mt-1"><Elapsed ms={queryElapsed} /></div>
      </div>
    );

  if (!queryResult) return null;

  const { columns, rows, row_count } = queryResult;

  if (columns.length === 0)
    return (
      <div className="flex flex-col items-center gap-1 py-6 text-xs text-muted-foreground">
        <span>Query executed successfully</span>
        <Elapsed ms={queryElapsed} />
      </div>
    );

  return (
    <div className="flex flex-1 flex-col overflow-hidden">
      <div className="flex shrink-0 items-center gap-3 border-y border-border bg-sidebar/50 px-3 py-0.5">
        <span className="text-[10px] text-muted-foreground">{row_count} row{row_count !== 1 && "s"}</span>
        <Elapsed ms={queryElapsed} />
      </div>
      <div className="flex-1 overflow-auto">
        <table className="w-full border-collapse text-xs">
          <thead className="sticky top-0 z-10 bg-sidebar">
            <tr>
              {columns.map((col) => (
                <th key={col} className="whitespace-nowrap border-b border-r border-border px-2 py-1 text-left font-medium text-muted-foreground">{col}</th>
              ))}
            </tr>
          </thead>
          <tbody>
            {rows.map((row, ri) => (
              <tr key={ri} className="hover:bg-accent/50">
                {row.map((val, ci) => (
                  <td key={ci} className={`whitespace-nowrap border-b border-r border-border/50 px-2 py-0.5 ${cellClass(val)}`}>{formatValue(val)}</td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}

export default function QueryTab() {
  const [sql, setSql] = useState("");
  const { queryLoading } = useSyncExternalStore(subscribe, getSnapshot);
  const trimmed = sql.trim();

  const runQuery = () => { if (trimmed) executeQuery(trimmed); };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if ((e.metaKey || e.ctrlKey) && e.key === "Enter") { e.preventDefault(); runQuery(); }
  };

  return (
    <div className="flex h-full flex-col">
      <div className="flex shrink-0 flex-col border-b border-border">
        <div className="flex items-center justify-between px-3 py-1">
          <span className="text-[10px] font-medium uppercase tracking-wider text-muted-foreground/50">Query</span>
          <Button size="xs" variant="ghost" className="gap-1" onClick={runQuery} disabled={queryLoading || !trimmed}>
            <Play className="h-3 w-3" />
            Run
            <kbd className="ml-1 text-[9px] text-muted-foreground/50">{navigator.platform.includes("Mac") ? "⌘" : "Ctrl"}↵</kbd>
          </Button>
        </div>
        <textarea
          value={sql}
          onChange={(e) => setSql(e.target.value)}
          onKeyDown={handleKeyDown}
          rows={5}
          className="resize-none bg-transparent px-3 pb-2 font-mono text-xs text-foreground outline-none placeholder:text-muted-foreground/30"
          placeholder="SELECT * FROM ..."
          spellCheck={false}
        />
      </div>
      <ResultsGrid />
    </div>
  );
}
