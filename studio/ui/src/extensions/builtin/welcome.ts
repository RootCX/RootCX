import { lazy } from "react";
import { LayoutDashboard } from "lucide-react";
import { views } from "../studio";

export const activate = () =>
  views.register("welcome", {
    title: "Welcome",
    icon: LayoutDashboard,
    defaultZone: "editor",
    component: lazy(() => import("@/components/panels/welcome-panel")),
  });
