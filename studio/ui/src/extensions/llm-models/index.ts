import { lazy } from "react";
import { Sparkles } from "lucide-react";
import { views } from "@/core/studio";

export const activate = () => {
  views.register("llm-models", { title: "LLM Models", icon: Sparkles, defaultZone: "sidebar", component: lazy(() => import("./panel")) });
};
