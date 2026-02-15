import { lazy } from "react";
import { FileText } from "lucide-react";
import { views } from "../studio";

export const activate = () =>
  views.register("output", {
    title: "Output",
    icon: FileText,
    defaultZone: "bottom",
    component: lazy(() => import("@/components/panels/output-panel")),
  });
