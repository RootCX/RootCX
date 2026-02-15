import { lazy, type ComponentType } from "react";
import type { LucideIcon } from "lucide-react";
import { FolderOpen, Hammer, Terminal, FileText, LayoutDashboard } from "lucide-react";
import type { ZoneId } from "@/components/layout/layout-store";

export interface ViewDefinition {
  id: string;
  title: string;
  icon: LucideIcon;
  defaultZone: ZoneId;
  component: React.LazyExoticComponent<ComponentType>;
}

export const views: ViewDefinition[] = [
  { id: "explorer", title: "Explorer", icon: FolderOpen, defaultZone: "sidebar", component: lazy(() => import("./explorer-panel")) },
  { id: "forge", title: "AI Forge", icon: Hammer, defaultZone: "sidebar", component: lazy(() => import("./forge-panel")) },
  { id: "welcome", title: "Welcome", icon: LayoutDashboard, defaultZone: "editor", component: lazy(() => import("./welcome-panel")) },
  { id: "console", title: "Console", icon: Terminal, defaultZone: "bottom", component: lazy(() => import("./console-panel")) },
  { id: "output", title: "Output", icon: FileText, defaultZone: "bottom", component: lazy(() => import("./output-panel")) },
];

export const viewMap = Object.fromEntries(views.map((v) => [v.id, v])) as Record<string, ViewDefinition>;
