import { lazy } from "react";
import { Settings } from "lucide-react";
import { views } from "@/core/studio";

export const activate = () =>
  views.register("settings", {
    title: "Settings",
    icon: Settings,
    defaultZone: "sidebar",
    component: lazy(() => import("./panel")),
  });
