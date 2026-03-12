import { lazy } from "react";
import { Bot } from "lucide-react";
import { views, commands } from "@/core/studio";
import { sendMessage, abortSession } from "./store";

export const activate = () => {
  views.register("forge", {
    title: "AI Forge",
    icon: Bot,
    defaultZone: "editor",
    defaultActive: true,
    component: lazy(() => import("./panel")),
  });

  commands.register("forge.send", {
    title: "Send Message",
    category: "AI Forge",
    handler: (prompt: unknown) => {
      if (typeof prompt === "string") sendMessage(prompt);
    },
  });

  commands.register("forge.abort", {
    title: "Abort Session",
    category: "AI Forge",
    handler: abortSession,
  });
};
