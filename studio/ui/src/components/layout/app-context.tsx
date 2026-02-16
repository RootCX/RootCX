import { createContext, useContext, useState, useCallback, useEffect, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { workspace } from "@/core/studio";

interface ProjectContext {
  projectPath: string | null;
  openProject: (path: string) => void;
}

const ProjectCtx = createContext<ProjectContext | null>(null);

export function ProjectProvider({ children }: { children: ReactNode }) {
  const [projectPath, setProjectPath] = useState<string | null>(null);

  const openProject = useCallback((path: string) => {
    setProjectPath(path);
    workspace.projectPath = path;
  }, []);
  workspace.openProject = openProject;

  useEffect(() => {
    if (projectPath) {
      invoke("sync_manifest", { projectPath }).catch(console.error);
    }
  }, [projectPath]);

  return (
    <ProjectCtx.Provider value={{ projectPath, openProject }}>
      {children}
    </ProjectCtx.Provider>
  );
}

export function useProjectContext(): ProjectContext {
  const ctx = useContext(ProjectCtx);
  if (!ctx) {
    throw new Error("useProjectContext must be used within ProjectProvider");
  }
  return ctx;
}
