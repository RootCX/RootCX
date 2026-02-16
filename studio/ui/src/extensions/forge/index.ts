import { lazy } from "react";
import { Bot } from "lucide-react";
import { views, commands, statusBar } from "@/core/studio";
import { sendMessage, abortSession } from "./store";
import { ForgeStatus } from "./status";

export const activate = () => {
  views.register("forge", {
    title: "AI Forge",
    icon: Bot,
    defaultZone: "sidebar",
    component: lazy(() => import("./panel")),
  });

  commands.register("forge.send", {
    title: "Send Message",
    category: "AI Forge",
    handler: (prompt: unknown) => {
      if (typeof prompt !== "string") return;
      sendMessage(prompt);
    },
  });

  commands.register("forge.abort", {
    title: "Abort Session",
    category: "AI Forge",
    handler: () => abortSession(),
  });

  statusBar.register("forge.status", {
    alignment: "left",
    priority: 10,
    component: ForgeStatus,
  });
};
