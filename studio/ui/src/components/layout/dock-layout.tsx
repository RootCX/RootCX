import { Suspense, lazy, useEffect, useRef, useMemo } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { ask } from "@tauri-apps/plugin-dialog";
import { ResizablePanelGroup, ResizablePanel, ResizableHandle } from "@/components/ui/resizable";
import { StatusBar } from "./status-bar";
import { PanelContainer } from "./panel-container";
import { ActivityBar } from "./activity-bar";
import { ProjectProvider, useProjectContext } from "./app-context";
import { LayoutProvider, useLayout, buildDefaultState, type ZoneId } from "./layout-store";
import { useViews } from "@/core/hooks";
import { views as viewRegistry, executeCommand, workspace, layout } from "@/core/studio";
import { CommandPaletteOverlay } from "@/extensions/command-palette/palette";
import { showAISetupDialog } from "@/components/ai-setup-dialog";
import { aiConfigStore } from "@/lib/ai-models";

const WelcomePanel = lazy(() => import("@/extensions/welcome/panel"));

function useEventListeners(dispatch: React.Dispatch<Parameters<typeof dispatch>[0]>) {
  useEffect(() => {
    const win = getCurrentWebviewWindow();
    const subs = [
      win.listen<string>("toggle-view", (e) => dispatch({ type: "TOGGLE_VIEW", viewId: e.payload })),
      win.listen("run", () => executeCommand("rootcx.run")),
      win.listen("reset-layout", async () => {
        if (await ask("Reset all views to their default positions?", { title: "Reset Layout", kind: "warning", okLabel: "Reset", cancelLabel: "Cancel" }))
          dispatch({ type: "RESET", defaultState: buildDefaultState(viewRegistry.getAll()) });
      }),
      win.listen("file:open-folder", () => executeCommand("project.open")),
      win.listen("file:create-project", () => executeCommand("project.create")),
      win.listen<string>("file:open-recent", (e) => {
        if (workspace.projectPath) invoke("create_window", { projectPath: e.payload }).catch(console.error);
        else workspace.openProject(e.payload);
      }),
    ];
    return () => { subs.forEach((s) => s.then((fn) => fn())); };
  }, [dispatch]);
}

function useAISetupPrompt() {
  const prompted = useRef(false);
  useEffect(() => {
    const check = () => {
      if (prompted.current) return;
      aiConfigStore.refresh().then(() => {
        if (!aiConfigStore.isLoaded() || prompted.current) return;
        prompted.current = true;
        if (!aiConfigStore.getSnapshot()) showAISetupDialog();
      });
    };
    const unlisten = listen("runtime-booted", check);
    check();
    return () => { unlisten.then((fn) => fn()); };
  }, []);
}

function Shell() {
  const { state, dispatch } = useLayout();
  const { projectPath } = useProjectContext();

  useEffect(() => { layout.dispatch = dispatch; workspace.projectPath = projectPath; }, [dispatch, projectPath]);
  useEffect(() => { invoke("sync_view_menu", { hidden: [...state.hidden] }).catch(() => {}); }, [state.hidden]);

  useEventListeners(dispatch);
  useAISetupPrompt();

  if (!projectPath) {
    return (
      <div className="h-screen w-screen overflow-hidden">
        <Suspense fallback={null}><WelcomePanel /></Suspense>
      </div>
    );
  }

  const zoneVisible = (zone: ZoneId) => state.zones[zone].some((id) => !state.hidden.has(id));
  const hasSidebar = zoneVisible("sidebar");
  const hasRight = zoneVisible("right");
  const hasBottom = zoneVisible("bottom");

  return (
    <div className="flex h-screen w-screen overflow-hidden">
      <ActivityBar />
      <div className="flex min-w-0 flex-1 flex-col overflow-hidden">
        <ResizablePanelGroup orientation="vertical" className="flex-1 overflow-hidden">
          <ResizablePanel id="top-area" defaultSize={hasBottom ? "70%" : "100%"}>
            <ResizablePanelGroup orientation="horizontal" className="h-full overflow-hidden">
              {hasSidebar && (<>
                <ResizablePanel id="sidebar" defaultSize="40%" minSize="10%" maxSize="60%" className="bg-sidebar">
                  <PanelContainer zone="sidebar" />
                </ResizablePanel>
                <ResizableHandle />
              </>)}
              <ResizablePanel id="main" defaultSize={hasSidebar || hasRight ? "40%" : "100%"}>
                <PanelContainer zone="editor" />
              </ResizablePanel>
              {hasRight && (<>
                <ResizableHandle />
                <ResizablePanel id="right" defaultSize="20%" minSize="3%" maxSize="40%" className="bg-sidebar">
                  <PanelContainer zone="right" />
                </ResizablePanel>
              </>)}
            </ResizablePanelGroup>
          </ResizablePanel>
          {hasBottom && (<>
            <ResizableHandle />
            <ResizablePanel id="bottom" defaultSize="30%" minSize="5%" maxSize="60%" className="bg-sidebar">
              <PanelContainer zone="bottom" />
            </ResizablePanel>
          </>)}
        </ResizablePanelGroup>
        <StatusBar />
        <CommandPaletteOverlay />
      </div>
    </div>
  );
}

export function DockLayout() {
  const views = useViews();
  const defaultState = useMemo(() => buildDefaultState(views), [views]);
  const validIds = useMemo(() => new Set(views.map((v) => v.id)), [views]);

  return (
    <ProjectProvider>
      <LayoutProvider defaultState={defaultState} validIds={validIds}>
        <Shell />
      </LayoutProvider>
    </ProjectProvider>
  );
}
