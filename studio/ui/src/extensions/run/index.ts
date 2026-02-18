import { invoke } from "@tauri-apps/api/core";
import { commands, workspace, layout } from "@/core/studio";

export function activate() {
  commands.register("rootcx.run", {
    title: "Run Project",
    category: "Project",
    handler: async () => {
      layout.showView("console");
      const pp = workspace.projectPath;
      if (!pp) return;

      try { await invoke("sync_manifest", { projectPath: pp }); } catch { }

      try {
        const entries = await invoke<{ name: string; is_dir: boolean }[]>("read_dir", { path: pp });
        if (entries.some((e) => e.name === "backend" && e.is_dir))
          await invoke("deploy_backend", { projectPath: pp });
      } catch { }

      try {
        const config = await invoke<{ command: string }>("read_launch_config", { projectPath: pp });
        await invoke("terminal_write", { data: config.command + "\n" });
      } catch {
        await invoke("init_launch_config", { projectPath: pp });
      }
    },
  });
}
