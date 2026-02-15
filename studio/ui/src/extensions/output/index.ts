import { lazy } from "react";
import { FileText } from "lucide-react";
import { views } from "@/core/studio";

export const activate = () =>
  views.register("output", {
    title: "Output",
    icon: FileText,
    defaultZone: "bottom",
    component: lazy(() => import("./panel")),
  });
