import { useSyncExternalStore, useState, useEffect } from "react";
import { subscribe, getSnapshot, uninstallAgent } from "@/extensions/agents/store";
import { subscribe as subscribeTools, getSnapshot as getToolsSnapshot, loadProject as loadToolsProject } from "@/extensions/agent-tools/store";
import { openAgentChat } from "@/extensions/agents";
import { Trash2, Database, Wrench, FolderOpen, Hammer, type LucideIcon } from "lucide-react";
import { ask } from "@tauri-apps/plugin-dialog";
import { views } from "@/core/studio";
import { cn } from "@/lib/utils";
import { useLayout, type ZoneId, type LayoutState } from "./layout-store";
import { useProjectContext } from "./app-context";
import { Tooltip, TooltipTrigger, TooltipContent, TooltipProvider } from "@/components/ui/tooltip";

const INDICATOR = "absolute left-0 top-1/2 h-6 w-0.5 -translate-y-1/2 rounded-r bg-foreground";
const BTN = "relative flex h-12 w-12 select-none items-center justify-center text-muted-foreground/50 transition-colors hover:text-muted-foreground";

interface NavItem { id: string; icon: LucideIcon; label: string; desc: string; zone: ZoneId }

const NAV_ITEMS: NavItem[] = [
  { id: "explorer", icon: FolderOpen, label: "Explorer", desc: "Browse project files", zone: "sidebar" },
  { id: "forge", icon: Hammer, label: "AI Forge", desc: "Chat with AI assistant", zone: "editor" },
  { id: "database", icon: Database, label: "Database", desc: "Browse schemas and tables", zone: "sidebar" },
];

const TOOLS_ITEM: NavItem = { id: "agent-tools", icon: Wrench, label: "Agent Tools", desc: "Configure agent tool access", zone: "sidebar" };

const isVisible = (state: LayoutState, id: string) =>
  Object.values(state.zones).some((ids) => ids.includes(id)) && !state.hidden.has(id);

export function ActivityBar() {
  const { agents } = useSyncExternalStore(subscribe, getSnapshot);
  const { isAgent } = useSyncExternalStore(subscribeTools, getToolsSnapshot);
  const { projectPath } = useProjectContext();

  useEffect(() => { if (projectPath) loadToolsProject(projectPath) }, [projectPath]);

  const [activeId, setActiveId] = useState<string | null>(null);
  const [menu, setMenu] = useState<{ appId: string; x: number; y: number } | null>(null);
  const { state, dispatch } = useLayout();

  const items = isAgent ? [...NAV_ITEMS, TOOLS_ITEM] : NAV_ITEMS;

  const toggle = ({ id, zone }: NavItem) => {
    if (isVisible(state, id)) dispatch({ type: "TOGGLE_VIEW", viewId: id });
    else dispatch({ type: "SHOW_VIEW", viewId: id, zone });
  };

  return (
    <TooltipProvider delayDuration={300}>
      <div className="flex w-12 shrink-0 flex-col items-center border-r border-border bg-sidebar">
        {items.map((item) => {
          const visible = isVisible(state, item.id);
          const Icon = item.icon;
          return (
            <Tooltip key={item.id}>
              <TooltipTrigger asChild>
                <button className={cn(BTN, visible && "text-foreground")} onClick={() => toggle(item)}>
                  {visible && <span className={INDICATOR} />}
                  <Icon className="h-5 w-5" />
                </button>
              </TooltipTrigger>
              <TooltipContent side="right" sideOffset={4}>
                <div className="text-xs font-semibold">{item.label}</div>
                <div className="text-[10px] text-muted-foreground">{item.desc}</div>
              </TooltipContent>
            </Tooltip>
          );
        })}

        {agents.length > 0 && <div className="my-1 h-px w-6 bg-border" />}

        {agents.map((a) => (
          <Tooltip key={a.app_id}>
            <TooltipTrigger asChild>
              <button
                className={cn(BTN, "text-[18px] font-bold", activeId === a.app_id && "text-foreground")}
                onClick={() => { setActiveId(a.app_id); openAgentChat(a.app_id, a.name); }}
                onContextMenu={(e) => { e.preventDefault(); setMenu({ appId: a.app_id, x: e.clientX, y: e.clientY }); }}
              >
                {activeId === a.app_id && <span className={INDICATOR} />}
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
          <div className="fixed inset-0 z-40" onClick={() => setMenu(null)} onContextMenu={(e) => { e.preventDefault(); setMenu(null); }} />
          <div className="fixed z-50 min-w-[160px] rounded-[5px] border border-[#454545] bg-[#252526] p-[4px] shadow-[0_2px_8px_rgba(0,0,0,0.5)]" style={{ left: menu.x, top: menu.y }}>
            <button
              className="flex w-full items-center gap-2 rounded-[3px] px-2 py-[3px] text-[13px] text-[#cc6b6b] hover:bg-[#3a1d1d] hover:text-[#f48771]"
              onClick={async () => {
                const { appId } = menu;
                setMenu(null);
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
