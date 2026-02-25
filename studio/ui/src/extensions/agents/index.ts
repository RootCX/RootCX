import { lazy } from "react";
import { Bot } from "lucide-react";
import { views, commands, layout } from "@/core/studio";
import { startPolling, clearChat } from "./store";

const panels = new Map<string, React.LazyExoticComponent<React.ComponentType>>();

function panelFor(appId: string) {
  let p = panels.get(appId);
  if (!p) {
    p = lazy(() => import("./panel").then((m) => ({ default: () => m.default({ appId }) })));
    panels.set(appId, p);
  }
  return p;
}

export function openAgentChat(appId: string, name?: string) {
  const id = `agent-chat:${appId}`;
  if (!views.get(id))
    views.register(id, {
      title: name ?? appId, icon: Bot, defaultZone: "editor", component: panelFor(appId),
      closeable: true,
      onClose: () => { views.unregister(id); clearChat(appId); },
    });
  layout.showView(id);
}

export function activate() {
  startPolling();
  commands.register("agents.openChat", {
    title: "Open Agent Chat",
    category: "Agents",
    handler: (appId: unknown, name?: unknown) => {
      if (typeof appId === "string") openAgentChat(appId, typeof name === "string" ? name : undefined);
    },
  });
}
