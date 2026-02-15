import { Suspense, useEffect, useRef, useCallback } from "react";
import { Tabs, TabsList, TabsTrigger, TabsContent } from "@/components/ui/tabs";
import { viewMap, type ViewDefinition } from "@/components/panels/registry";
import { useLayout, type ZoneId } from "./layout-store";

// ── Pointer-based drag (module-level, shared across instances) ──

let drag: { viewId: string; startX: number; startY: number; active: boolean } | null = null;
const zoneRefs = new Map<ZoneId, HTMLElement>();

function hitZone(x: number, y: number): ZoneId | null {
  for (const [id, el] of zoneRefs) {
    const r = el.getBoundingClientRect();
    if (r.width > 0 && r.height > 0 && x >= r.left && x <= r.right && y >= r.top && y <= r.bottom)
      return id;
  }
  return null;
}

function clearHighlights() {
  for (const el of zoneRefs.values()) el.classList.remove("drop-target");
}

// ── Component ──

export function PanelContainer({ zone }: { zone: ZoneId }) {
  const { state, dispatch } = useLayout();
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const el = ref.current;
    if (el) zoneRefs.set(zone, el);
    return () => { zoneRefs.delete(zone); };
  }, [zone]);

  const resolved: ViewDefinition[] = [];
  for (const id of state.zones[zone]) {
    if (!state.hidden.has(id) && viewMap[id]) resolved.push(viewMap[id]);
  }
  const activeId = state.active[zone];

  const startDrag = useCallback(
    (e: React.PointerEvent, viewId: string) => {
      e.preventDefault();
      drag = { viewId, startX: e.clientX, startY: e.clientY, active: false };

      const cleanup = () => {
        document.removeEventListener("pointermove", onMove);
        document.removeEventListener("pointerup", onUp);
        document.removeEventListener("keydown", onKey);
        document.body.style.cursor = "";
        clearHighlights();
        drag = null;
      };

      const onMove = (ev: PointerEvent) => {
        if (!drag) return;
        if (!drag.active) {
          const dx = ev.clientX - drag.startX;
          const dy = ev.clientY - drag.startY;
          if (dx * dx + dy * dy < 25) return;
          drag.active = true;
          document.body.style.cursor = "grabbing";
        }
        const target = hitZone(ev.clientX, ev.clientY);
        for (const [z, el] of zoneRefs) {
          el.classList.toggle("drop-target", z === target);
        }
      };

      const onUp = (ev: PointerEvent) => {
        const wasDrag = drag?.active;
        const vid = drag?.viewId;
        cleanup();
        if (wasDrag && vid) {
          const target = hitZone(ev.clientX, ev.clientY);
          if (target) dispatch({ type: "MOVE_VIEW", viewId: vid, toZone: target });
        } else {
          dispatch({ type: "SET_ACTIVE", zone, viewId });
        }
      };

      const onKey = (ev: KeyboardEvent) => {
        if (ev.key === "Escape") cleanup();
      };

      document.addEventListener("pointermove", onMove);
      document.addEventListener("pointerup", onUp);
      document.addEventListener("keydown", onKey);
    },
    [zone, dispatch],
  );

  if (resolved.length === 0) {
    return <div ref={ref} className="h-full" />;
  }

  return (
    <div ref={ref} className="flex h-full flex-col">
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
                onPointerDown={(e) => startDrag(e, view.id)}
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
