import { useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
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
    const unlisten = listen<string>("toggle-view", (e) => {
      dispatch({ type: "TOGGLE_VIEW", viewId: e.payload });
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [dispatch]);

  const hasVisible = (zone: "sidebar" | "editor" | "bottom") =>
    state.zones[zone].some((id) => !state.hidden.has(id));

  const sidebarOpen = hasVisible("sidebar");
  const bottomOpen = hasVisible("bottom");

  return (
    <div className="flex h-screen w-screen flex-col overflow-hidden">
      <ResizablePanelGroup orientation="horizontal" className="flex-1 overflow-hidden">
        {sidebarOpen && (
          <>
            <ResizablePanel
              id="sidebar"
              defaultSize="25%"
              minSize="15%"
              maxSize="40%"
              className="bg-sidebar"
            >
              <PanelContainer zone="sidebar" />
            </ResizablePanel>
            <ResizableHandle />
          </>
        )}

        <ResizablePanel id="main" defaultSize={sidebarOpen ? "75%" : "100%"}>
          <ResizablePanelGroup orientation="vertical">
            <ResizablePanel
              id="editor"
              defaultSize={bottomOpen ? "70%" : "100%"}
              minSize="30%"
            >
              <PanelContainer zone="editor" />
            </ResizablePanel>

            {bottomOpen && (
              <>
                <ResizableHandle />
                <ResizablePanel
                  id="bottom"
                  defaultSize="30%"
                  minSize="10%"
                  maxSize="60%"
                >
                  <PanelContainer zone="bottom" />
                </ResizablePanel>
              </>
            )}
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
