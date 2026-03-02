import { lazy } from "react";
import { Container } from "lucide-react";
import { views } from "@/core/studio";

export const activate = () => {
  views.register("workers", { title: "Workers", icon: Container, defaultZone: "sidebar", component: lazy(() => import("./panel")) });
};
