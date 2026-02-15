import { lazy } from "react";
import { LayoutDashboard } from "lucide-react";
import { views } from "@/core/studio";

export const activate = () =>
  views.register("welcome", {
    title: "Welcome",
    icon: LayoutDashboard,
    defaultZone: "editor",
    component: lazy(() => import("./panel")),
  });
