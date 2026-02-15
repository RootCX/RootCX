import { Suspense, useState, useCallback } from "react";
import { Tabs, TabsList, TabsTrigger, TabsContent } from "@/components/ui/tabs";
import { viewMap, type ViewDefinition } from "@/components/panels/registry";
import { useLayout, type ZoneId } from "./layout-store";
import { cn } from "@/lib/utils";

let draggedViewId: string | null = null;

export function PanelContainer({ zone }: { zone: ZoneId }) {
  const { state, dispatch } = useLayout();
  const [dragOver, setDragOver] = useState(false);

  const resolved: ViewDefinition[] = [];
  for (const id of state.zones[zone]) {
    if (!state.hidden.has(id) && viewMap[id]) resolved.push(viewMap[id]);
  }
  const activeId = state.active[zone];

  const onDragStart = useCallback((e: React.DragEvent, viewId: string) => {
    draggedViewId = viewId;
    e.dataTransfer.effectAllowed = "move";
    e.dataTransfer.setData("text/plain", viewId);
  }, []);

  const onDragEnd = useCallback(() => {
    draggedViewId = null;
    setDragOver(false);
  }, []);

  const onDrop = useCallback(
    (e: React.DragEvent) => {
      e.preventDefault();
      setDragOver(false);
      if (draggedViewId) {
        dispatch({ type: "MOVE_VIEW", viewId: draggedViewId, toZone: zone });
        draggedViewId = null;
      }
    },
    [dispatch, zone],
  );

  const onDragOver = useCallback((e: React.DragEvent) => {
    if (draggedViewId) {
      e.preventDefault();
      e.dataTransfer.dropEffect = "move";
      setDragOver(true);
    }
  }, []);

  const onDragLeave = useCallback((e: React.DragEvent) => {
    if (!e.currentTarget.contains(e.relatedTarget as Node)) setDragOver(false);
  }, []);

  if (resolved.length === 0) return null;

  return (
    <div
      className={cn("flex h-full flex-col", dragOver && "ring-2 ring-inset ring-primary/50")}
      onDragOver={onDragOver}
      onDrop={onDrop}
      onDragLeave={onDragLeave}
    >
      <Tabs
        value={activeId ?? resolved[0].id}
        onValueChange={(id) => dispatch({ type: "SET_ACTIVE", zone, viewId: id })}
        className="flex h-full flex-col"
      >
        <div className="flex h-8 shrink-0 items-center border-b border-border bg-panel">
          <TabsList>
            {resolved.map((view) => (
              <TabsTrigger
                key={view.id}
                value={view.id}
                draggable
                onDragStart={(e) => onDragStart(e, view.id)}
                onDragEnd={onDragEnd}
              >
                {view.title}
              </TabsTrigger>
            ))}
          </TabsList>
        </div>
        {resolved.map((view) => (
          <TabsContent key={view.id} value={view.id} className="flex-1 overflow-auto">
            <Suspense
              fallback={
                <div className="flex items-center justify-center p-8 text-sm text-muted-foreground">
                  Loading...
                </div>
              }
            >
              <view.component />
            </Suspense>
          </TabsContent>
        ))}
      </Tabs>
    </div>
  );
}
