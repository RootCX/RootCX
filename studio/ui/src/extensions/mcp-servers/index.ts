import { lazy } from "react";
import { Cable } from "lucide-react";
import { views, commands } from "@/core/studio";
import { refresh } from "./store";

export const activate = () => {
  views.register("mcp-servers", {
    title: "MCP Servers",
    icon: Cable,
    defaultZone: "sidebar",
    component: lazy(() => import("./panel")),
  });

  commands.register("mcp.refresh", {
    title: "Refresh MCP Servers",
    category: "MCP",
    handler: () => refresh(),
  });
};
