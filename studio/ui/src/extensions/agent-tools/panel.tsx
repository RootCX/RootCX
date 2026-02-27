import { useSyncExternalStore } from "react";
import { useProjectContext } from "@/components/layout/app-context";
import { ListRow } from "@/components/ui/list-row";
import { subscribe, getSnapshot, type ToolEntry } from "./store";

function ToolRow({ tool }: { tool: ToolEntry }) {
  return (
    <ListRow className="px-3 py-1.5">
      <div className="min-w-0 flex-1">
        <div className="text-xs font-medium">{tool.name}</div>
        <div className="truncate text-[10px] text-muted-foreground">{tool.description.split("\n")[0]}</div>
      </div>
    </ListRow>
  );
}

export default function AgentToolsPanel() {
  const { projectPath } = useProjectContext();
  const { tools, isAgent, loading } = useSyncExternalStore(subscribe, getSnapshot);

  if (!projectPath) {
    return <div className="flex h-full items-center justify-center p-6 text-xs text-muted-foreground">No project opened</div>;
  }
  if (!isAgent && !loading) {
    return <div className="flex h-full items-center justify-center p-6 text-xs text-muted-foreground">Not an agent project</div>;
  }

  return (
    <div className="flex h-full flex-col">
      <div className="flex shrink-0 items-center border-b border-border px-3 py-1.5">
        <span className="text-xs font-medium uppercase tracking-wider text-muted-foreground">Agent Tools</span>
      </div>
      {loading && tools.length === 0 && (
        <div className="flex animate-pulse items-center justify-center p-6 text-xs text-muted-foreground">Loading tools...</div>
      )}
      <div className="flex-1 overflow-y-auto py-1">
        {tools.map((t) => <ToolRow key={t.name} tool={t} />)}
      </div>
    </div>
  );
}
