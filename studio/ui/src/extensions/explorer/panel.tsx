import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { useProjectContext } from "@/components/layout/app-context";
import { Button } from "@/components/ui/button";
import { FolderOpen, ChevronRight, ChevronDown, File, Folder } from "lucide-react";
import { cn } from "@/lib/utils";

interface DirEntry {
  name: string;
  path: string;
  is_dir: boolean;
}

interface TreeNodeState {
  expanded: boolean;
  children: DirEntry[] | null;
}

function FileTreeNode({
  entry,
  depth,
}: {
  entry: DirEntry;
  depth: number;
}) {
  const [state, setState] = useState<TreeNodeState>({
    expanded: false,
    children: null,
  });

  const toggle = useCallback(async () => {
    if (!entry.is_dir) return;

    if (!state.expanded && state.children === null) {
      try {
        const children = await invoke<DirEntry[]>("read_dir", {
          path: entry.path,
        });
        setState({ expanded: true, children });
      } catch {
        setState({ expanded: false, children: [] });
      }
    } else {
      setState((s) => ({ ...s, expanded: !s.expanded }));
    }
  }, [entry.path, entry.is_dir, state.expanded, state.children]);

  return (
    <div>
      <button
        onClick={toggle}
        className={cn(
          "flex w-full items-center gap-1 px-1 py-0.5 text-left text-xs hover:bg-accent",
          !entry.is_dir && "cursor-default",
        )}
        style={{ paddingLeft: `${depth * 12 + 4}px` }}
      >
        {entry.is_dir ? (
          state.expanded ? (
            <ChevronDown className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
          ) : (
            <ChevronRight className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
          )
        ) : (
          <span className="w-3.5 shrink-0" />
        )}
        {entry.is_dir ? (
          <Folder className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
        ) : (
          <File className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
        )}
        <span className="truncate">{entry.name}</span>
      </button>
      {entry.is_dir && state.expanded && state.children && (
        <div>
          {state.children.map((child) => (
            <FileTreeNode key={child.path} entry={child} depth={depth + 1} />
          ))}
        </div>
      )}
    </div>
  );
}

export default function ExplorerPanel() {
  const { projectPath, openProject } = useProjectContext();
  const [entries, setEntries] = useState<DirEntry[]>([]);
  const [error, setError] = useState<string | null>(null);

  const handleOpenFolder = useCallback(async () => {
    const selected = await open({ directory: true, multiple: false });
    if (selected) {
      openProject(selected);
    }
  }, [openProject]);

  useEffect(() => {
    if (!projectPath) {
      setEntries([]);
      return;
    }
    (async () => {
      try {
        const result = await invoke<DirEntry[]>("read_dir", {
          path: projectPath,
        });
        setEntries(result);
        setError(null);
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
      }
    })();
  }, [projectPath]);

  if (!projectPath) {
    return (
      <div className="flex flex-col items-center justify-center gap-3 p-6">
        <p className="text-xs text-muted-foreground">No folder opened</p>
        <Button size="sm" variant="outline" onClick={handleOpenFolder}>
          <FolderOpen className="mr-1.5 h-3.5 w-3.5" />
          Open Folder
        </Button>
      </div>
    );
  }

  const folderName = projectPath.split("/").pop() || projectPath;

  return (
    <div className="flex h-full flex-col">
      <div className="flex shrink-0 items-center justify-between border-b border-border px-3 py-1.5">
        <span className="truncate text-xs font-medium uppercase tracking-wider text-muted-foreground">
          {folderName}
        </span>
        <Button size="icon" variant="ghost" className="h-6 w-6" onClick={handleOpenFolder}>
          <FolderOpen className="h-3.5 w-3.5" />
        </Button>
      </div>

      {error && (
        <div className="px-3 py-2 text-xs text-red-400">{error}</div>
      )}

      <div className="flex-1 overflow-y-auto py-1">
        {entries.map((entry) => (
          <FileTreeNode key={entry.path} entry={entry} depth={0} />
        ))}
      </div>
    </div>
  );
}
