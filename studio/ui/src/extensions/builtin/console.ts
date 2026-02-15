import { lazy } from "react";
import { Terminal } from "lucide-react";
import { views } from "../studio";

export const activate = () =>
  views.register("console", {
    title: "Console",
    icon: Terminal,
    defaultZone: "bottom",
    component: lazy(() => import("@/components/panels/console-panel")),
  });
