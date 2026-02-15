import { lazy } from "react";
import { Terminal } from "lucide-react";
import { views } from "@/core/studio";

export const activate = () =>
  views.register("console", {
    title: "Console",
    icon: Terminal,
    defaultZone: "bottom",
    component: lazy(() => import("./panel")),
  });
