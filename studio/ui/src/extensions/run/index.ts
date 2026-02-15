import { invoke } from "@tauri-apps/api/core";
import { commands, workspace, layout } from "@/core/studio";

export function activate() {
  commands.register("rootcx.run", {
    title: "Run Project",
    handler: async () => {
      layout.showView("console");

      if (!workspace.projectPath) {
        await invoke("terminal_write", { data: "\r\n\x1b[33m⚠ Open a project first.\x1b[0m\r\n" });
        return;
      }

      try {
        const config = await invoke<{ command: string }>("read_launch_config", {
          projectPath: workspace.projectPath,
        });
        await invoke("terminal_write", { data: config.command + "\n" });
      } catch {
        await invoke("init_launch_config", { projectPath: workspace.projectPath });
        await invoke("terminal_write", {
          data: "\r\n\x1b[36m✦ Created .rootcx/launch.json — edit it and press Run again.\x1b[0m\r\n",
        });
      }
    },
  });
}
