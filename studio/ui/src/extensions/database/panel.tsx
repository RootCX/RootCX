import { useState, useEffect, useSyncExternalStore } from "react";
import { ChevronRight, ChevronDown, Table2, Columns3, RefreshCw, Plus } from "lucide-react";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { layout, executeCommand } from "@/core/studio";
import { useProjectContext } from "@/components/layout/app-context";
import {
  subscribe,
  getSnapshot,
  loadProject,
  refresh,
  queryTable,
  type TableInfo,
  type ColumnInfo,
} from "./store";

function ColumnNode({ col, depth }: { col: ColumnInfo; depth: number }) {
  return (
    <div
      className="flex w-full items-center gap-1 px-1 py-0.5 text-xs text-muted-foreground"
      style={{ paddingLeft: `${depth * 12 + 4}px` }}
    >
      <span className="w-3.5 shrink-0" />
      <Columns3 className="h-3 w-3 shrink-0 text-muted-foreground/60" />
      <span className="truncate">
        {col.column_name}
        <span className="ml-1 text-[10px] text-muted-foreground/50">
          {col.data_type}
          {!col.is_nullable && " NOT NULL"}
        </span>
      </span>
    </div>
  );
}

function TableNode({ table, depth }: { table: TableInfo; depth: number }) {
  const [expanded, setExpanded] = useState(false);

  return (
    <div>
      <button
        onClick={() => setExpanded((e) => !e)}
        onDoubleClick={() => {
          queryTable(table.table_name);
          layout.showView("db-query");
        }}
        className="flex w-full items-center gap-1 px-1 py-0.5 text-left text-xs hover:bg-accent"
        style={{ paddingLeft: `${depth * 12 + 4}px` }}
      >
        {expanded ? (
          <ChevronDown className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
        ) : (
          <ChevronRight className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
        )}
        <Table2 className="h-3.5 w-3.5 shrink-0 text-blue-400" />
        <span className="truncate">{table.table_name}</span>
        <span className="ml-auto shrink-0 pr-2 text-[10px] text-muted-foreground/50">
          {table.columns.length} cols
          {table.row_estimate > 0 && ` · ~${table.row_estimate}`}
        </span>
      </button>
      {expanded && (
        <div>
          {table.columns.map((col) => (
            <ColumnNode key={col.column_name} col={col} depth={depth + 1} />
          ))}
        </div>
      )}
    </div>
  );
}

export default function DatabasePanel() {
  const { projectPath } = useProjectContext();
  const { appId, tables, loading, error } = useSyncExternalStore(subscribe, getSnapshot);

  useEffect(() => {
    if (projectPath) loadProject(projectPath);
  }, [projectPath]);

  if (!projectPath) {
    return (
      <div className="flex h-full items-center justify-center p-6 text-xs text-muted-foreground">
        No project opened
      </div>
    );
  }

  return (
    <div className="flex h-full flex-col">
      <div className="flex shrink-0 items-center justify-between border-b border-border px-3 py-1.5">
        <span className="truncate text-xs font-medium uppercase tracking-wider text-muted-foreground">
          {appId ?? "Database"}
        </span>
        <div className="flex items-center gap-0.5">
          <Button
            size="sm"
            variant="secondary"
            className="h-6 gap-1 px-2 text-[10px]"
            onClick={() => executeCommand("database.newQuery")}
          >
            <Plus className="h-3 w-3" />
            New Query
          </Button>
          <Button size="icon" variant="ghost" className="h-6 w-6" onClick={() => refresh()}>
            <RefreshCw className={cn("h-3.5 w-3.5", loading && "animate-spin")} />
          </Button>
        </div>
      </div>

      {error && <div className="px-3 py-2 text-xs text-red-400">{error}</div>}

      {loading && tables.length === 0 && (
        <div className="flex animate-pulse items-center justify-center p-6 text-xs text-muted-foreground">
          Loading schema...
        </div>
      )}

      {!loading && !error && tables.length === 0 && appId && (
        <div className="flex items-center justify-center p-6 text-xs text-muted-foreground">
          No tables
        </div>
      )}

      <div className="flex-1 overflow-y-auto py-1">
        {tables.map((t) => (
          <TableNode key={t.table_name} table={t} depth={0} />
        ))}
      </div>
    </div>
  );
}
