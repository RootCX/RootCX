import { lazy } from "react";
import { Bot } from "lucide-react";
import { ask } from "@tauri-apps/plugin-dialog";
import { listen } from "@tauri-apps/api/event";
import { views, commands, layout, executeCommand } from "@/core/studio";
import { notify } from "@/core/notifications";
import { clearChat, checkDeployment, markUndeployed } from "./store";

const panels = new Map<string, React.LazyExoticComponent<React.ComponentType>>();

function panelFor(appId: string, name?: string) {
  let p = panels.get(appId);
  if (!p) {
    p = lazy(() => import("./panel").then((m) => ({ default: () => m.default({ appId, name }) })));
    panels.set(appId, p);
  }
  return p;
}

let currentAppId: string | null = null;

export function openAgentChat(appId: string, name?: string) {
  const id = `agent-chat:${appId}`;
  currentAppId = appId;
  if (!views.get(id))
    views.register(id, {
      title: name ?? appId, icon: Bot, defaultZone: "editor", component: panelFor(appId, name),
      closeable: true,
      onClose: async () => {
        if (await ask("Close this conversation?", { title: "Close", kind: "warning", okLabel: "Close", cancelLabel: "Cancel" })) {
          views.unregister(id); clearChat(appId);
          if (currentAppId === appId) currentAppId = null;
        }
      },
    });
  layout.showView(id);
  checkDeployment(appId).then((ok) => {
    if (!ok) notify("agent-not-deployed", "Run the project to deploy the agent", "warning", {
      label: "Run",
      run: () => executeCommand("rootcx.run"),
    });
  });
}

export function activate() {
  commands.register("agents.openChat", {
    title: "Open Agent Chat",
    category: "Agents",
    handler: (appId: unknown, name?: unknown) => {
      if (typeof appId === "string") openAgentChat(appId, typeof name === "string" ? name : undefined);
    },
  });

  const recheck = () => { if (currentAppId) checkDeployment(currentAppId); };
  listen("runtime-booted", recheck);
  listen("run-started", recheck);
  listen("run-exited", markUndeployed);
}
