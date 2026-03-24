import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import { commands, workspace, layout } from "@/core/studio";
import { readLaunchConfig, runPreLaunch } from "./pre-launch";

export function activate() {
  listen("run-exited", () => {
    emit("run-output", "\r\n\x1b[36m[process exited]\x1b[0m\r\n");
  });

  commands.register("rootcx.run", {
    title: "Run Project",
    category: "Project",
    handler: async () => {
      layout.showView("output");
      const pp = workspace.projectPath;
      if (!pp) return;

      const config = await readLaunchConfig(pp);
      if (!config) return;

      if (!(await runPreLaunch(config.preLaunch, pp))) return;

      try {
        await invoke("run_app", { projectPath: pp });
      } catch {
        await invoke("init_launch_config", { projectPath: pp });
      }
    },
  });

  commands.register("rootcx.applyMigrations", {
    title: "Apply Migrations",
    category: "Project",
    handler: async () => {
      layout.showView("output");
      const pp = workspace.projectPath;
      if (!pp) return;

      emit("run-output", "\x1b[36m[migrations] Deploying backend...\x1b[0m\r\n");
      try {
        await invoke("deploy_backend", { projectPath: pp });
        emit("run-output", "\x1b[32m[migrations] Applied successfully.\x1b[0m\r\n");
      } catch (e) {
        emit("run-output", `\x1b[31m[migrations] ${e instanceof Error ? e.message : e}\x1b[0m\r\n`);
      }
    },
  });
}
