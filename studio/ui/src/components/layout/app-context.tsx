import { createContext, useContext, useState, useCallback, type ReactNode } from "react";
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
