import { lazy } from "react";
import { Hammer } from "lucide-react";
import { views, commands, statusBar, workspace } from "@/core/studio";
import { sendMessage, stopBuild } from "./store";
import { ForgePhaseStatus } from "./status";

export const activate = () => {
  views.register("forge", {
    title: "AI Forge",
    icon: Hammer,
    defaultZone: "sidebar",
    component: lazy(() => import("./panel")),
  });

  commands.register("forge.send", {
    title: "Forge: Send Message",
    handler: (prompt: unknown, appId?: unknown) => {
      if (typeof prompt !== "string" || !workspace.projectPath) return;
      sendMessage(prompt, workspace.projectPath, appId as string | undefined);
    },
  });

  commands.register("forge.stop", {
    title: "Forge: Stop Build",
    handler: () => stopBuild(),
  });

  statusBar.register("forge.phase", {
    alignment: "left",
    priority: 10,
    component: ForgePhaseStatus,
  });
};
