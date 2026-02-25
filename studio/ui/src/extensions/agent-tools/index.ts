import { lazy } from "react";
import { Wrench } from "lucide-react";
import { views, commands, layout } from "@/core/studio";

export const activate = () => {
  views.register("agent-tools", {
    title: "Agent Tools",
    icon: Wrench,
    defaultZone: "sidebar",
    component: lazy(() => import("./panel")),
  });

  commands.register("agentTools.show", {
    title: "Show Agent Tools",
    category: "Agent",
    handler: () => layout.showView("agent-tools"),
  });
};
