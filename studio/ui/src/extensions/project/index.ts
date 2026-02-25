import { open, ask } from "@tauri-apps/plugin-dialog";
import { invoke } from "@tauri-apps/api/core";
import { commands, workspace } from "@/core/studio";
import { showScaffoldWizard } from "./scaffold-wizard";

async function openFolder() {
  const selected = await open({ directory: true });
  if (!selected) return;

  if (workspace.projectPath) {
    const newWindow = await ask("Open in a new window or this window?", {
      title: "Open Folder",
      kind: "info",
      okLabel: "New Window",
      cancelLabel: "This Window",
    });
    if (newWindow) {
      await invoke("create_window", { projectPath: selected });
      return;
    }
  }
  workspace.openProject(selected);
}

async function createProject() {
  const result = await showScaffoldWizard();
  if (!result) return;

  await invoke("scaffold_project", { ...result });

  if (workspace.projectPath) {
    const newWindow = await ask("Open in a new window or this window?", {
      title: "Create Project",
      kind: "info",
      okLabel: "New Window",
      cancelLabel: "This Window",
    });
    if (newWindow) {
      await invoke("create_window", { projectPath: result.path });
      return;
    }
  }
  workspace.openProject(result.path);
}

export function activate() {
  commands.register("project.open", { title: "Open Folder", category: "File", handler: openFolder });
  commands.register("project.create", { title: "Create Project", category: "File", handler: createProject });
}
