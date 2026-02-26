import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { executeCommand, workspace } from "@/core/studio";
import { Button } from "@/components/ui/button";
import { ListRow } from "@/components/ui/list-row";
import { Logo } from "@/components/logo";
import { FolderOpen, Plus, Clock } from "lucide-react";

function RecentProjects() {
  const [recents, setRecents] = useState<RecentProject[]>([]);

  useEffect(() => {
    invoke<RecentProject[]>("get_recent_projects").then(setRecents).catch(() => {});
  }, []);

  if (recents.length === 0) return null;

  return (
    <div className="mt-8 w-full max-w-sm">
      <h2 className="mb-3 flex items-center gap-2 text-xs font-medium uppercase tracking-wider text-muted-foreground/50">
        <Clock className="h-3 w-3" />
        Recent Projects
      </h2>
      <div className="flex flex-col gap-0.5">
        {recents.map((project) => (
          <ListRow key={project.path} onClick={() => workspace.openProject(project.path)} className="flex-col items-start px-3 py-2">
            <span className="text-sm font-medium text-foreground/80">{project.name}</span>
            <span className="truncate text-xs text-muted-foreground/50">{project.path}</span>
          </ListRow>
        ))}
      </div>
    </div>
  );
}

export default function WelcomePanel() {
  return (
    <div className="relative flex h-full items-center justify-center overflow-hidden">
      <Logo className="pointer-events-none absolute h-[60%] max-h-[400px] text-white/[0.03]" />
      <div className="z-10 flex flex-col items-center gap-6">
        <h1 className="text-2xl font-semibold tracking-tight text-muted-foreground/60">RootCX Studio</h1>
        <div className="flex gap-3">
          <Button variant="outline" onClick={() => executeCommand("project.open")}>
            <FolderOpen className="mr-2 h-4 w-4" />
            Open Folder
          </Button>
          <Button variant="outline" onClick={() => executeCommand("project.create")}>
            <Plus className="mr-2 h-4 w-4" />
            Create Project
          </Button>
        </div>
        <RecentProjects />
      </div>
    </div>
  );
}
