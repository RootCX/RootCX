import type { ComponentType } from "react";
import type { LucideIcon } from "lucide-react";
import type { ZoneId, Action } from "@/components/layout/layout-store";
import { Registry, type Disposable } from "./registry";
import { registerKeybinding } from "./keybindings";

export interface View {
  title: string;
  icon: LucideIcon;
  defaultZone: ZoneId;
  component: React.LazyExoticComponent<ComponentType>;
  closeable?: boolean;
  onClose?: () => void;
}

export interface Command {
  title: string;
  category?: string;
  keybinding?: string;
  handler: (...args: unknown[]) => void | Promise<void>;
}

export interface StatusBarItem {
  alignment: "left" | "right";
  priority: number;
  component: ComponentType;
}

class CommandRegistry extends Registry<Command> {
  register(id: string, item: Command): Disposable {
    const disposable = super.register(id, item);
    if (item.keybinding) {
      const kbDisposable = registerKeybinding(id, item.keybinding);
      return {
        dispose: () => {
          kbDisposable.dispose();
          disposable.dispose();
        },
      };
    }
    return disposable;
  }
}

export const views = new Registry<View>();
export const commands = new CommandRegistry();
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
    const zone = views.get(id)?.defaultZone;
    this.dispatch?.({ type: "SHOW_VIEW", viewId: id, zone });
  },
};
