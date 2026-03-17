import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { verifySchema, syncManifest } from "@/core/api";
import { showMigrationDialog } from "./migration-dialog";

interface LaunchConfig {
  preLaunch: string[];
}

export async function readLaunchConfig(projectPath: string): Promise<LaunchConfig | null> {
  try {
    return await invoke<LaunchConfig>("read_launch_config", { projectPath });
  } catch {
    await invoke("init_launch_config", { projectPath });
    return null;
  }
}

export async function runPreLaunch(steps: string[], projectPath: string): Promise<boolean> {
  for (const step of steps) {
    if (!(await executeStep(step, projectPath))) return false;
  }
  return true;
}

async function readManifest(projectPath: string): Promise<unknown> {
  const raw = await invoke<string>("read_file", { path: `${projectPath}/manifest.json` });
  return JSON.parse(raw);
}

function logError(step: string, err: unknown): false {
  emit("run-output", `\x1b[31m[${step}] ${err instanceof Error ? err.message : err}\x1b[0m\r\n`);
  return false;
}

async function executeStep(step: string, projectPath: string): Promise<boolean> {
  switch (step) {
    case "verify_schema": {
      try {
        const result = await verifySchema(await readManifest(projectPath));
        if (!result.compliant) return await showMigrationDialog(result.changes);
      } catch (e) { return logError("verify_schema", e); }
      return true;
    }
    case "sync_manifest": {
      try {
        await syncManifest(await readManifest(projectPath));
      } catch (e) { return logError("sync_manifest", e); }
      return true;
    }
    case "install_deps": {
      try {
        await invoke("install_deps", { projectPath });
      } catch (e) { return logError("install_deps", e); }
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
    case "publish_frontend": {
      try {
        emit("run-output", "\x1b[36m[publish] Building frontend...\x1b[0m\r\n");
        const url = await invoke<string>("deploy_frontend", { projectPath });
        emit("run-output", `\x1b[32m[publish] ${url}\x1b[0m\r\n`);
      } catch (e) { return logError("publish_frontend", e); }
      return true;
    }
    default:
      return true;
  }
}
