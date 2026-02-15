import { useState } from "react";
import { TooltipProvider } from "@/components/ui/tooltip";
import {
  ResizablePanelGroup,
  ResizablePanel,
  ResizableHandle,
} from "@/components/ui/resizable";
import { ActivityBar } from "./activity-bar";
import { StatusBar } from "./status-bar";
import { PanelContainer } from "./panel-container";
import { ProjectProvider } from "./app-context";
import { getPanelsByPosition } from "@/components/panels/registry";

const sidebarPanels = getPanelsByPosition("sidebar");
const editorPanels = getPanelsByPosition("editor");
const bottomPanels = getPanelsByPosition("bottom");

export function DockLayout() {
  const [activeSidebarId, setActiveSidebarId] = useState<string | null>(
    sidebarPanels[0]?.id ?? null,
  );
  const [activeEditorId, setActiveEditorId] = useState(
    editorPanels[0]?.id ?? "",
  );
  const [activeBottomId, setActiveBottomId] = useState(
    bottomPanels[0]?.id ?? "",
  );

  const handleSidebarToggle = (id: string) => {
    setActiveSidebarId((prev) => (prev === id ? null : id));
  };

  const sidebarOpen = activeSidebarId !== null;

  return (
    <TooltipProvider delayDuration={300}>
      <ProjectProvider>
        <div className="flex h-screen w-screen flex-col overflow-hidden">
          <div className="flex flex-1 overflow-hidden">
            {/* Activity Bar */}
            <ActivityBar
              sidebarPanels={sidebarPanels}
              activeSidebarId={activeSidebarId}
              onToggle={handleSidebarToggle}
            />

            {/* Main content area */}
            <ResizablePanelGroup orientation="horizontal" className="flex-1">
              {/* Sidebar */}
              {sidebarOpen && (
                <>
                  <ResizablePanel
                    id="sidebar"
                    defaultSize="25%"
                    minSize="15%"
                    maxSize="40%"
                    className="bg-sidebar"
                  >
                    <PanelContainer
                      panels={sidebarPanels}
                      activeId={activeSidebarId!}
                      onTabChange={setActiveSidebarId}
                    />
                  </ResizablePanel>
                  <ResizableHandle />
                </>
              )}

              {/* Editor + Bottom */}
              <ResizablePanel id="main" defaultSize={sidebarOpen ? "75%" : "100%"}>
                <ResizablePanelGroup orientation="vertical">
                  {/* Editor Area */}
                  <ResizablePanel id="editor" defaultSize="70%" minSize="30%">
                    <PanelContainer
                      panels={editorPanels}
                      activeId={activeEditorId}
                      onTabChange={setActiveEditorId}
                    />
                  </ResizablePanel>

                  <ResizableHandle />

                  {/* Bottom Panel */}
                  <ResizablePanel id="bottom" defaultSize="30%" minSize="10%" maxSize="60%">
                    <PanelContainer
                      panels={bottomPanels}
                      activeId={activeBottomId}
                      onTabChange={setActiveBottomId}
                    />
                  </ResizablePanel>
                </ResizablePanelGroup>
              </ResizablePanel>
            </ResizablePanelGroup>
          </div>

          {/* Status Bar */}
          <StatusBar />
        </div>
      </ProjectProvider>
    </TooltipProvider>
  );
}
