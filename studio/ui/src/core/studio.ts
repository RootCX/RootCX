import type { ComponentType } from "react";
import type { LucideIcon } from "lucide-react";
import type { ZoneId, Action } from "@/components/layout/layout-store";
import { Registry } from "./registry";

export interface View {
  title: string;
  icon: LucideIcon;
  defaultZone: ZoneId;
  component: React.LazyExoticComponent<ComponentType>;
}

export interface Command {
  title: string;
  handler: (...args: unknown[]) => void | Promise<void>;
}

export interface StatusBarItem {
  alignment: "left" | "right";
  priority: number;
  component: ComponentType;
}

export const views = new Registry<View>();
export const commands = new Registry<Command>();
export const statusBar = new Registry<StatusBarItem>();

export function executeCommand(id: string, ...args: unknown[]) {
  const cmd = commands.get(id);
  if (!cmd) throw new Error(`Unknown command: ${id}`);
  return cmd.handler(...args);
}

export const workspace = {
  projectPath: null as string | null,
  openProject: (_path: string) => {},
};

export const layout = {
  dispatch: null as React.Dispatch<Action> | null,
  showView(id: string) {
    this.dispatch?.({ type: "SHOW_VIEW", viewId: id });
  },
};
