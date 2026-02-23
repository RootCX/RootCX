import { useSyncExternalStore } from "react";
import { subscribe, getSnapshot } from "@/extensions/agents/store";
import { openAgentChat } from "@/extensions/agents";
import { Tooltip, TooltipTrigger, TooltipContent, TooltipProvider } from "@/components/ui/tooltip";

export function ActivityBar() {
  const { agents } = useSyncExternalStore(subscribe, getSnapshot);
  if (!agents.length) return null;

  return (
    <TooltipProvider delayDuration={200}>
      <div className="flex w-12 shrink-0 flex-col items-center gap-2 border-r border-border bg-sidebar py-2">
        {agents.map((a) => (
          <Tooltip key={a.app_id}>
            <TooltipTrigger asChild>
              <button
                className="flex h-9 w-9 items-center justify-center rounded-lg bg-accent text-sm font-bold text-foreground transition-colors hover:bg-primary hover:text-primary-foreground"
                onClick={() => openAgentChat(a.app_id, a.name)}
              >
                {a.name[0].toUpperCase()}
              </button>
            </TooltipTrigger>
            <TooltipContent side="right">
              <div className="text-xs font-semibold">{a.name}</div>
              {a.description && <div className="text-[10px] text-muted-foreground">{a.description}</div>}
            </TooltipContent>
          </Tooltip>
        ))}
      </div>
    </TooltipProvider>
  );
}
