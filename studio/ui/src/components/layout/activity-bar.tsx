import { useEffect, useState, useMemo, useSyncExternalStore } from "react";
import { Cable, Container, Database, FolderOpen, Hammer, KeyRound, Plug, Shield, LogOut, ServerOff, type LucideIcon } from "lucide-react";
import { useAuth, logout, disconnect } from "@/core/auth";
import { cn } from "@/lib/utils";
import { useLayout, type ZoneId, type LayoutState } from "./layout-store";
import { Tooltip, TooltipTrigger, TooltipContent, TooltipProvider } from "@/components/ui/tooltip";
import { subscribe, getSnapshot, checkAdmin } from "@/extensions/workers/store";

const INDICATOR = "absolute left-0 top-1/2 h-6 w-0.5 -translate-y-1/2 rounded-r bg-foreground";
const BTN = "relative flex h-12 w-12 select-none items-center justify-center text-muted-foreground/50 transition-colors hover:text-muted-foreground";

interface NavItem { id: string; icon: LucideIcon; label: string; desc: string; zone: ZoneId }

const BASE_NAV: NavItem[] = [
  { id: "explorer", icon: FolderOpen, label: "Explorer", desc: "Browse project files", zone: "sidebar" },
  { id: "forge", icon: Hammer, label: "AI Forge", desc: "Chat with AI assistant", zone: "editor" },
  { id: "database", icon: Database, label: "Database", desc: "Browse schemas and tables", zone: "sidebar" },
  { id: "security", icon: Shield, label: "Security", desc: "Manage roles and permissions", zone: "sidebar" },
  { id: "integrations", icon: Plug, label: "Integrations", desc: "Connect external services", zone: "sidebar" },
  { id: "secrets", icon: KeyRound, label: "Platform Secrets", desc: "Manage encrypted env variables", zone: "sidebar" },
  { id: "mcp-servers", icon: Cable, label: "MCP Servers", desc: "Connect MCP tool servers", zone: "sidebar" },
];
const WORKERS_NAV: NavItem = { id: "workers", icon: Container, label: "Workers", desc: "Manage app workers", zone: "sidebar" };

const isActive = (state: LayoutState, id: string) =>
  Object.values(state.active).includes(id) && !state.hidden.has(id);

export function ActivityBar() {
  const [userMenu, setUserMenu] = useState<{ x: number; y: number } | null>(null);
  const { state, dispatch } = useLayout();
  const { user } = useAuth();
  const { isCoreAdmin } = useSyncExternalStore(subscribe, getSnapshot);
  useEffect(() => { checkAdmin(); }, [user?.id]);
  const navItems = useMemo(() => isCoreAdmin ? [...BASE_NAV, WORKERS_NAV] : BASE_NAV, [isCoreAdmin]);

  return (
    <TooltipProvider delayDuration={300}>
      <div className="flex w-12 shrink-0 flex-col items-center border-r border-border bg-sidebar">
        {navItems.map(({ id, icon: Icon, label, desc, zone }) => {
          const active = isActive(state, id);
          return (
            <Tooltip key={id}>
              <TooltipTrigger asChild>
                <button className={cn(BTN, active && "text-foreground")} onClick={() => dispatch({ type: "SHOW_VIEW", viewId: id, zone })}>
                  {active && <span className={INDICATOR} />}
                  <Icon className="h-5 w-5" />
                </button>
              </TooltipTrigger>
              <TooltipContent side="right" sideOffset={4}>
                <div className="text-xs font-semibold">{label}</div>
                <div className="text-[10px] text-muted-foreground">{desc}</div>
              </TooltipContent>
            </Tooltip>
          );
        })}

        {user && (
          <div className="mt-auto mb-2">
            <Tooltip>
              <TooltipTrigger asChild>
                <button
                  className="relative flex h-8 w-8 cursor-pointer items-center justify-center rounded-full border border-border/60 text-xs font-medium text-muted-foreground transition-colors hover:border-border hover:text-foreground"
                  onClick={(e) => { e.preventDefault(); setUserMenu({ x: e.clientX, y: e.clientY }); }}
                >
                  {(user.displayName || user.username)[0].toUpperCase()}
                  <span className="absolute bottom-0 right-0 h-2 w-2 rounded-full border border-sidebar bg-emerald-500" />
                </button>
              </TooltipTrigger>
              <TooltipContent side="right" sideOffset={4}>
                <div className="text-xs font-semibold">{user.displayName || user.username}</div>
                {user.email && <div className="text-[10px] text-muted-foreground">{user.email}</div>}
              </TooltipContent>
            </Tooltip>
          </div>
        )}
      </div>

      {userMenu && user && (
        <>
          <div className="fixed inset-0 z-40" onClick={() => setUserMenu(null)} onContextMenu={(e) => { e.preventDefault(); setUserMenu(null); }} />
          <div className="fixed z-50 min-w-[160px] rounded-[5px] border border-[#454545] bg-[#252526] p-[4px] shadow-[0_2px_8px_rgba(0,0,0,0.5)]" style={{ left: userMenu.x, top: userMenu.y - 40 }}>
            <div className="px-2 py-1 text-[11px] text-muted-foreground/60">{user.displayName || user.username}</div>
            <button
              className="flex w-full items-center gap-2 rounded-[3px] px-2 py-[3px] text-[13px] text-foreground hover:bg-[#2a2d2e]"
              onClick={() => { setUserMenu(null); logout(); }}
            >
              <LogOut className="h-3 w-3" /> Sign out
            </button>
            <button
              className="flex w-full items-center gap-2 rounded-[3px] px-2 py-[3px] text-[13px] text-foreground hover:bg-[#2a2d2e]"
              onClick={() => { setUserMenu(null); disconnect(); }}
            >
              <ServerOff className="h-3 w-3" /> Switch server
            </button>
          </div>
        </>
      )}
    </TooltipProvider>
  );
}
