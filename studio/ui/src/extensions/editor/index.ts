import { lazy } from "react";
import { Code2 } from "lucide-react";
import { views, commands, statusBar } from "@/core/studio";
import { openFile, saveFile, closeTab, getSnapshot } from "./store";
import { CursorStatus, LanguageStatus } from "./status";

export const activate = () => {
  views.register("editor", {
    title: "Editor",
    icon: Code2,
    defaultZone: "editor",
    component: lazy(() => import("./panel")),
  });

  commands.register("editor.open", {
    title: "Open File in Editor",
    handler: (path: unknown) => openFile(path as string),
  });

  commands.register("editor.save", {
    title: "Save File",
    handler: () => saveFile(),
  });

  commands.register("editor.closeTab", {
    title: "Close Editor Tab",
    handler: (path?: unknown) => {
      const target = (path as string) ?? getSnapshot().activeTab;
      if (target) closeTab(target);
    },
  });

  statusBar.register("editor.language", {
    alignment: "right",
    priority: 20,
    component: LanguageStatus,
  });

  statusBar.register("editor.cursor", {
    alignment: "right",
    priority: 10,
    component: CursorStatus,
  });
};
