import { useSyncExternalStore, useState } from "react";
import { subscribe, getSnapshot } from "@/extensions/agents/store";
import { openAgentChat } from "@/extensions/agents";
import { cn } from "@/lib/utils";
import { Tooltip, TooltipTrigger, TooltipContent, TooltipProvider } from "@/components/ui/tooltip";

export function ActivityBar() {
  const { agents } = useSyncExternalStore(subscribe, getSnapshot);
  const [activeId, setActiveId] = useState<string | null>(null);

  if (!agents.length) return null;

  return (
    <TooltipProvider delayDuration={300}>
      <div className="flex w-12 shrink-0 flex-col items-center border-r border-border bg-sidebar">
        {agents.map((a) => (
          <Tooltip key={a.app_id}>
            <TooltipTrigger asChild>
              <button
                className={cn(
                  "relative flex h-12 w-12 items-center justify-center text-[18px] font-bold text-muted-foreground/50 transition-colors hover:text-muted-foreground",
                  activeId === a.app_id && "text-foreground",
                )}
                onClick={() => { setActiveId(a.app_id); openAgentChat(a.app_id, a.name); }}
              >
                {activeId === a.app_id && (
                  <span className="absolute left-0 top-1/2 h-6 w-0.5 -translate-y-1/2 rounded-r bg-foreground" />
                )}
                {a.name[0].toUpperCase()}
              </button>
            </TooltipTrigger>
            <TooltipContent side="right" sideOffset={4}>
              <div className="text-xs font-semibold">{a.name}</div>
              {a.description && <div className="text-[10px] text-muted-foreground">{a.description}</div>}
            </TooltipContent>
          </Tooltip>
        ))}
      </div>
    </TooltipProvider>
  );
}
