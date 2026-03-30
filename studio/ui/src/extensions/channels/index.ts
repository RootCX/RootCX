import { lazy } from "react";
import { MessageCircle } from "lucide-react";
import { views, commands } from "@/core/studio";
import { refresh } from "./store";

export const activate = () => {
  views.register("channels", {
    title: "Channels",
    icon: MessageCircle,
    defaultZone: "sidebar",
    component: lazy(() => import("./panel")),
  });

  commands.register("channels.refresh", {
    title: "Refresh Channels",
    category: "Channels",
    handler: () => refresh(),
  });
};
