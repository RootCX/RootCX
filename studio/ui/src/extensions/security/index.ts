import { lazy } from "react";
import { Shield } from "lucide-react";
import { views, commands, layout } from "@/core/studio";
import { refresh } from "./store";

export const activate = () => {
  views.register("security", { title: "Security", icon: Shield, defaultZone: "sidebar", component: lazy(() => import("./panel")) });
  commands.register("security.refresh", { title: "Refresh Security", category: "Security", handler: () => refresh() });
  commands.register("security.show", { title: "Show Security Panel", category: "Security", handler: () => layout.showView("security") });
};
