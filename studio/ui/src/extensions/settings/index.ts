import { lazy } from "react";
import { Settings } from "lucide-react";
import { views, commands, layout } from "@/core/studio";
import { showAISetupDialog } from "@/components/ai-setup-dialog";

export const activate = () => {
  views.register("settings", {
    title: "Settings",
    icon: Settings,
    defaultZone: "sidebar",
    component: lazy(() => import("./panel")),
  });

  commands.register("settings.open", {
    title: "Open Settings",
    category: "Settings",
    keybinding: "Mod+,",
    handler: () => { layout.showView("settings"); },
  });

  commands.register("ai.setup", {
    title: "Configure AI Provider",
    category: "Settings",
    handler: () => { showAISetupDialog(); },
  });
};
