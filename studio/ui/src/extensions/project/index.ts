import { open, save } from "@tauri-apps/plugin-dialog";
import { invoke } from "@tauri-apps/api/core";
import { commands, workspace } from "@/core/studio";

async function openFolder() {
  const selected = await open({ directory: true });
  if (selected) workspace.openProject(selected);
}

async function createProject() {
  const path = await save({ title: "Create Project", defaultPath: "my-project" });
  if (!path) return;
  await invoke("scaffold_project", { path, name: path.split("/").pop() ?? path });
  workspace.openProject(path);
}

export function activate() {
  commands.register("project.open", { title: "Open Folder", category: "File", handler: openFolder });
  commands.register("project.create", { title: "Create Project", category: "File", handler: createProject });
}
