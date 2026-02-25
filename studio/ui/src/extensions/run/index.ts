import { invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import { commands, workspace, layout } from "@/core/studio";
import { dismiss } from "@/core/notifications";
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

function logError(step: string, err: unknown): false {
  emit("run-output", `\x1b[31m[${step}] ${err instanceof Error ? err.message : err}\x1b[0m\r\n`);
  return false;
}

async function executeStep(step: string, projectPath: string): Promise<boolean> {
  switch (step) {
    case "verify_schema": {
      try {
        const result = await invoke<SchemaVerification>("verify_schema", { projectPath });
        if (!result.compliant) {
          return await showMigrationDialog(result.changes);
        }
      } catch (e) { return logError("verify_schema", e); }
      return true;
    }
    case "sync_manifest": {
      try {
        await invoke("sync_manifest", { projectPath });
        dismiss("agent-tools-changed");
      } catch (e) { return logError("sync_manifest", e); }
      return true;
    }
    case "deploy_backend": {
      try {
        const entries = await invoke<{ name: string; is_dir: boolean }[]>("read_dir", { path: projectPath });
        if (entries.some((e) => e.is_dir && e.name === "backend"))
          await invoke("deploy_backend", { projectPath });
      } catch (e) { return logError("deploy_backend", e); }
      return true;
    }
    default:
      return true;
  }
}
