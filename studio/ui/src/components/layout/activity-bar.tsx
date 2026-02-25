import { useSyncExternalStore, useState } from "react";
import { subscribe, getSnapshot, uninstallAgent } from "@/extensions/agents/store";
import { openAgentChat } from "@/extensions/agents";
import { Trash2 } from "lucide-react";
import { ask } from "@tauri-apps/plugin-dialog";
import { views } from "@/core/studio";
import { cn } from "@/lib/utils";
import { Tooltip, TooltipTrigger, TooltipContent, TooltipProvider } from "@/components/ui/tooltip";

export function ActivityBar() {
  const { agents } = useSyncExternalStore(subscribe, getSnapshot);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [menu, setMenu] = useState<{ appId: string; x: number; y: number } | null>(null);

  if (!agents.length) return null;

  return (
    <TooltipProvider delayDuration={300}>
      <div className="flex w-12 shrink-0 flex-col items-center border-r border-border bg-sidebar">
        {agents.map((a) => (
          <Tooltip key={a.app_id}>
            <TooltipTrigger asChild>
              <button
                className={cn(
                  "relative flex h-12 w-12 select-none items-center justify-center text-[18px] font-bold text-muted-foreground/50 transition-colors hover:text-muted-foreground",
                  activeId === a.app_id && "text-foreground",
                )}
                onClick={() => { setActiveId(a.app_id); openAgentChat(a.app_id, a.name); }}
                onContextMenu={(e) => { e.preventDefault(); setMenu({ appId: a.app_id, x: e.clientX, y: e.clientY }); }}
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

      {menu && (
        <>
          <div className="fixed inset-0 z-40" onClick={() => setMenu(null)} onContextMenu={(e) => { e.preventDefault(); () => setMenu(null)(); }} />
          <div className="fixed z-50 min-w-[160px] rounded-[5px] border border-[#454545] bg-[#252526] p-[4px] shadow-[0_2px_8px_rgba(0,0,0,0.5)]" style={{ left: menu.x, top: menu.y }}>
            <button
              className="flex w-full items-center gap-2 rounded-[3px] px-2 py-[3px] text-[13px] text-[#cc6b6b] hover:bg-[#3a1d1d] hover:text-[#f48771]"
              onClick={async () => {
                const { appId } = menu;
                () => setMenu(null)();
                const name = agents.find((a) => a.app_id === appId)?.name ?? appId;
                if (await ask(`Delete "${name}"? This will undeploy the agent.`, { title: "Delete Agent", kind: "warning", okLabel: "Delete", cancelLabel: "Cancel" })) {
                  views.unregister(`agent-chat:${appId}`);
                  await uninstallAgent(appId).catch(console.error);
                  if (activeId === appId) setActiveId(null);
                }
              }}
            >
              <Trash2 className="h-3 w-3" /> Delete
            </button>
          </div>
        </>
      )}
    </TooltipProvider>
  );
}
