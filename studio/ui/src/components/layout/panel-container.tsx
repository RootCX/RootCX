import { Suspense } from "react";
import { Tabs, TabsList, TabsTrigger, TabsContent } from "@/components/ui/tabs";
import type { PanelDefinition } from "@/components/panels/registry";

interface PanelContainerProps {
  panels: PanelDefinition[];
  activeId: string;
  onTabChange: (id: string) => void;
}

export function PanelContainer({
  panels,
  activeId,
  onTabChange,
}: PanelContainerProps) {
  if (panels.length === 0) return null;

  return (
    <Tabs
      value={activeId}
      onValueChange={onTabChange}
      className="flex h-full flex-col"
    >
      <div className="flex h-8 shrink-0 items-center border-b border-border bg-panel">
        <TabsList>
          {panels.map((panel) => (
            <TabsTrigger key={panel.id} value={panel.id}>
              {panel.title}
            </TabsTrigger>
          ))}
        </TabsList>
      </div>
      {panels.map((panel) => (
        <TabsContent
          key={panel.id}
          value={panel.id}
          className="flex-1 overflow-auto"
        >
          <Suspense
            fallback={
              <div className="flex items-center justify-center p-8 text-sm text-muted-foreground">
                Loading...
              </div>
            }
          >
            <panel.component />
          </Suspense>
        </TabsContent>
      ))}
    </Tabs>
  );
}
