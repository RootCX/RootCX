import { open } from "@tauri-apps/plugin-dialog";
import { invoke } from "@tauri-apps/api/core";
import { commands, workspace } from "@/core/studio";
import { showScaffoldWizard } from "./scaffold-wizard";

async function openFolder() {
  const selected = await open({ directory: true });
  if (selected) workspace.openProject(selected);
}

async function createProject() {
  const result = await showScaffoldWizard();
  if (!result) return;
  await invoke("scaffold_project", { ...result });
  workspace.openProject(result.path);
}

export function activate() {
  commands.register("project.open", { title: "Open Folder", category: "File", handler: openFolder });
  commands.register("project.create", { title: "Create Project", category: "File", handler: createProject });
}
