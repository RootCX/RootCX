import { lazy } from "react";
import { FolderOpen } from "lucide-react";
import { views } from "@/core/studio";

export const activate = () =>
  views.register("explorer", {
    title: "Explorer",
    icon: FolderOpen,
    defaultZone: "sidebar",
    component: lazy(() => import("./panel")),
  });
