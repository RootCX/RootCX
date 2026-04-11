import { lazy } from "react";
import { Clock } from "lucide-react";
import { views } from "@/core/studio";

export const activate = () => {
  views.register("crons", {
    title: "Scheduled Jobs",
    icon: Clock,
    defaultZone: "sidebar",
    component: lazy(() => import("./panel")),
  });
};
