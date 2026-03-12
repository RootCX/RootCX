import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { commands, workspace, layout, statusBar } from "@/core/studio";
import { PublishButton } from "./publish-button";

export function activate() {
  commands.register("rootcx.publish", {
    title: "Publish Frontend",
    category: "Project",
    keybinding: "Mod+Shift+D",
    handler: async () => {
      const pp = workspace.projectPath;
      if (!pp) return;

      layout.showView("output");
      emit("publish-started", undefined);
      emit("run-output", "\x1b[36m[publish] Building frontend...\x1b[0m\r\n");

      try {
        const url = await invoke<string>("deploy_frontend", { projectPath: pp });
        emit("run-output", `\x1b[32m[publish] Frontend published!\x1b[0m\r\n`);
        emit("run-output", `\x1b[36m[publish] URL: ${url}\x1b[0m\r\n`);
      } catch (e) {
        emit("run-output", `\x1b[31m[publish] ${e instanceof Error ? e.message : e}\x1b[0m\r\n`);
      } finally {
        emit("publish-finished", undefined);
      }
    },
  });

  statusBar.register("rootcx.publish", {
    alignment: "right",
    priority: 99,
    component: PublishButton,
  });
}
