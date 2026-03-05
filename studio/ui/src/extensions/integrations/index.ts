import { lazy } from "react";
import { Plug } from "lucide-react";
import { views, commands } from "@/core/studio";
import { refresh } from "./store";

export const activate = () => {
  views.register("integrations", {
    title: "Integrations",
    icon: Plug,
    defaultZone: "sidebar",
    component: lazy(() => import("./panel")),
  });

  commands.register("integrations.refresh", {
    title: "Refresh Integrations",
    category: "Integrations",
    handler: () => refresh(),
  });
};
