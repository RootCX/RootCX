import { cn } from "@/lib/utils";
import {
  Tooltip,
  TooltipTrigger,
  TooltipContent,
} from "@/components/ui/tooltip";
import type { PanelDefinition } from "@/components/panels/registry";

interface ActivityBarProps {
  sidebarPanels: PanelDefinition[];
  activeSidebarId: string | null;
  onToggle: (id: string) => void;
}

export function ActivityBar({
  sidebarPanels,
  activeSidebarId,
  onToggle,
}: ActivityBarProps) {
  return (
    <div className="flex h-full w-12 shrink-0 flex-col items-center gap-1 border-r border-border bg-sidebar pt-2">
      {sidebarPanels.map((panel) => {
        const Icon = panel.icon;
        const isActive = activeSidebarId === panel.id;
        return (
          <Tooltip key={panel.id}>
            <TooltipTrigger asChild>
              <button
                onClick={() => onToggle(panel.id)}
                className={cn(
                  "flex h-10 w-10 items-center justify-center rounded-md text-muted-foreground transition-colors hover:text-foreground",
                  isActive &&
                    "bg-accent text-foreground border-l-2 border-primary",
                )}
              >
                <Icon className="h-5 w-5" />
              </button>
            </TooltipTrigger>
            <TooltipContent side="right">{panel.title}</TooltipContent>
          </Tooltip>
        );
      })}
    </div>
  );
}
