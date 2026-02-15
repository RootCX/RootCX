import { useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { ask } from "@tauri-apps/plugin-dialog";
import {
  ResizablePanelGroup,
  ResizablePanel,
  ResizableHandle,
} from "@/components/ui/resizable";
import { StatusBar } from "./status-bar";
import { PanelContainer } from "./panel-container";
import { ProjectProvider } from "./app-context";
import { LayoutProvider, useLayout, buildDefaultState } from "./layout-store";
import { views } from "@/components/panels/registry";

const defaultState = buildDefaultState(views);
const validIds = new Set(views.map((v) => v.id));

function Shell() {
  const { state, dispatch } = useLayout();

  useEffect(() => {
    invoke("sync_view_menu", { hidden: [...state.hidden] }).catch(() => {});
  }, [state.hidden]);

  useEffect(() => {
    const u1 = listen<string>("toggle-view", (e) => {
      dispatch({ type: "TOGGLE_VIEW", viewId: e.payload });
    });
    const u2 = listen("run", () => {
      dispatch({ type: "SHOW_VIEW", viewId: "console" });
    });
    const u3 = listen("reset-layout", async () => {
      const ok = await ask("Reset all views to their default positions?", {
        title: "Reset Layout",
        kind: "warning",
        okLabel: "Reset",
        cancelLabel: "Cancel",
      });
      if (ok) dispatch({ type: "RESET", defaultState });
    });
    return () => {
      u1.then((fn) => fn());
      u2.then((fn) => fn());
      u3.then((fn) => fn());
    };
  }, [dispatch]);

  return (
    <div className="flex h-screen w-screen flex-col overflow-hidden">
      <ResizablePanelGroup orientation="horizontal" className="flex-1 overflow-hidden">
        <ResizablePanel
          id="sidebar"
          defaultSize="20%"
          minSize="3%"
          maxSize="40%"
          className="bg-sidebar"
        >
          <PanelContainer zone="sidebar" />
        </ResizablePanel>
        <ResizableHandle />

        <ResizablePanel id="main" defaultSize="80%">
          <ResizablePanelGroup orientation="vertical">
            <ResizablePanel id="editor" defaultSize="70%" minSize="10%">
              <PanelContainer zone="editor" />
            </ResizablePanel>
            <ResizableHandle />
            <ResizablePanel id="bottom" defaultSize="30%" minSize="3%" maxSize="60%">
              <PanelContainer zone="bottom" />
            </ResizablePanel>
          </ResizablePanelGroup>
        </ResizablePanel>
      </ResizablePanelGroup>
      <StatusBar />
    </div>
  );
}

export function DockLayout() {
  return (
    <ProjectProvider>
      <LayoutProvider defaultState={defaultState} validIds={validIds}>
        <Shell />
      </LayoutProvider>
    </ProjectProvider>
  );
}
