import { emit } from "@tauri-apps/api/event";
import { commands, workspace, layout } from "@/core/studio";
import { readLaunchConfig, runPreLaunch } from "../run/pre-launch";

export function activate() {
  commands.register("rootcx.deploy", {
    title: "Deploy to Core",
    category: "Project",
    keybinding: "Mod+Shift+D",
    handler: async () => {
      layout.showView("output");
      const pp = workspace.projectPath;
      if (!pp) return;

      const config = await readLaunchConfig(pp);
      if (!config) return;

      emit("run-output", "\x1b[36m[deploy] Starting deployment...\x1b[0m\r\n");
      const ok = await runPreLaunch(config.preLaunch, pp);
      emit("run-output", ok
        ? "\x1b[32m[deploy] Deployed successfully.\x1b[0m\r\n"
        : "\x1b[31m[deploy] Deployment failed.\x1b[0m\r\n",
      );
    },
  });
}
