import { lazy } from "react";
import { KeyRound } from "lucide-react";
import { views } from "@/core/studio";

export const activate = () => {
  views.register("secrets", { title: "Platform Secrets", icon: KeyRound, defaultZone: "sidebar", component: lazy(() => import("./panel")) });
};
