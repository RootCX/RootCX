import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { commands, workspace, layout } from "@/core/studio";
import { showMigrationDialog } from "./migration-dialog";
import type { SchemaVerification } from "@/types";

interface LaunchConfig {
  preLaunch: string[];
}

export function activate() {
  listen("run-exited", () => {
    invoke("stop_deployed_worker").catch(() => {});
  });

  commands.register("rootcx.run", {
    title: "Run Project",
    category: "Project",
    handler: async () => {
      layout.showView("output");
      const pp = workspace.projectPath;
      if (!pp) return;

      let config: LaunchConfig;
      try {
        config = await invoke<LaunchConfig>("read_launch_config", { projectPath: pp });
      } catch {
        await invoke("init_launch_config", { projectPath: pp });
        return;
      }

      for (const step of config.preLaunch) {
        const ok = await executeStep(step, pp);
        if (!ok) return;
      }

      try {
        await invoke("run_app", { projectPath: pp });
      } catch {
        await invoke("init_launch_config", { projectPath: pp });
      }
    },
  });
}

async function executeStep(step: string, projectPath: string): Promise<boolean> {
  switch (step) {
    case "verify_schema": {
      try {
        const result = await invoke<SchemaVerification>("verify_schema", { projectPath });
        if (!result.compliant) {
          return await showMigrationDialog(result.changes);
        }
      } catch (e) { console.error("verify_schema failed:", e); }
      return true;
    }
    case "sync_manifest": {
      try { await invoke("sync_manifest", { projectPath }); } catch { }
      return true;
    }
    case "deploy_backend": {
      try { await invoke("deploy_backend", { projectPath }); } catch { }
      return true;
    }
    default:
      return true;
  }
}
