import { lazy, type ComponentType } from "react";
import type { LucideIcon } from "lucide-react";
import {
  FolderOpen,
  Hammer,
  Terminal,
  FileText,
  LayoutDashboard,
} from "lucide-react";

export type PanelPosition = "sidebar" | "editor" | "bottom";

export interface PanelDefinition {
  id: string;
  title: string;
  icon: LucideIcon;
  position: PanelPosition;
  component: React.LazyExoticComponent<ComponentType>;
}

export const panels: PanelDefinition[] = [
  {
    id: "explorer",
    title: "Explorer",
    icon: FolderOpen,
    position: "sidebar",
    component: lazy(() => import("./explorer-panel")),
  },
  {
    id: "forge",
    title: "AI Forge",
    icon: Hammer,
    position: "sidebar",
    component: lazy(() => import("./forge-panel")),
  },
  {
    id: "welcome",
    title: "Welcome",
    icon: LayoutDashboard,
    position: "editor",
    component: lazy(() => import("./welcome-panel")),
  },
  {
    id: "console",
    title: "Console",
    icon: Terminal,
    position: "bottom",
    component: lazy(() => import("./console-panel")),
  },
  {
    id: "output",
    title: "Output",
    icon: FileText,
    position: "bottom",
    component: lazy(() => import("./output-panel")),
  },
];

export function getPanelsByPosition(position: PanelPosition) {
  return panels.filter((p) => p.position === position);
}
