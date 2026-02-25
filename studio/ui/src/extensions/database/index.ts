import { lazy } from "react";
import { Database, SquareTerminal } from "lucide-react";
import { views, commands, layout } from "@/core/studio";
import { refresh } from "./store";

export const activate = () => {
  views.register("database", {
    title: "Database",
    icon: Database,
    defaultZone: "sidebar",
    component: lazy(() => import("./panel")),
  });

  views.register("db-query", {
    title: "Query",
    icon: SquareTerminal,
    defaultZone: "editor",
    closeable: true,
    component: lazy(() => import("./query-tab")),
  });

  commands.register("database.refresh", {
    title: "Refresh Database Browser",
    category: "Database",
    handler: () => refresh(),
  });

  commands.register("database.show", {
    title: "Show Database Browser",
    category: "Database",
    handler: () => layout.showView("database"),
  });

  commands.register("database.newQuery", {
    title: "New Query",
    category: "Database",
    handler: () => layout.showView("db-query"),
  });
};
