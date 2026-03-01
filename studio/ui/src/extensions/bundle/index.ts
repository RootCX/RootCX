import { invoke } from "@tauri-apps/api/core";
import { commands, workspace, layout, statusBar } from "@/core/studio";
import { BundleButton } from "./bundle-button";

export function activate() {
  commands.register("rootcx.bundle", {
    title: "Bundle for Distribution",
    category: "Project",
    keybinding: "Mod+Shift+B",
    handler: async () => {
      const pp = workspace.projectPath;
      if (!pp) return;
      layout.showView("output");
      await invoke("bundle_app", { projectPath: pp });
    },
  });

  statusBar.register("rootcx.bundle", {
    alignment: "right",
    priority: 100,
    component: BundleButton,
  });
}
