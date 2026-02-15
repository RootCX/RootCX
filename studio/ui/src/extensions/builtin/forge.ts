import { lazy } from "react";
import { Hammer } from "lucide-react";
import { views } from "../studio";

export const activate = () =>
  views.register("forge", {
    title: "AI Forge",
    icon: Hammer,
    defaultZone: "sidebar",
    component: lazy(() => import("@/components/panels/forge-panel")),
  });
