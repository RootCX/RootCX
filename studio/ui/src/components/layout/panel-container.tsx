import { Suspense, useEffect, useRef, useCallback } from "react";
import { Tabs, TabsList, TabsTrigger, TabsContent } from "@/components/ui/tabs";
import { X } from "lucide-react";
import { ask } from "@tauri-apps/plugin-dialog";
import { views, type View } from "@/core/studio";
import type { Entry } from "@/core/registry";
import { useLayout, type ZoneId } from "./layout-store";

// ── Pointer-based drag (module-level, shared across instances) ──

let drag: {
  viewId: string;
  startX: number;
  startY: number;
  active: boolean;
  originZone: ZoneId;
  originIndex: number;
  lastZone: ZoneId | null;
  lastIndex: number;
} | null = null;
const zoneRefs = new Map<ZoneId, HTMLElement>();

function hitZone(x: number, y: number): ZoneId | null {
  for (const [id, el] of zoneRefs) {
    const r = el.getBoundingClientRect();
    if (r.width > 0 && r.height > 0 && x >= r.left && x <= r.right && y >= r.top && y <= r.bottom)
      return id;
  }
  return null;
}

function hitIndex(zone: ZoneId, clientX: number, draggedId: string): number {
  const el = zoneRefs.get(zone);
  if (!el) return 0;
  const tabs = [...el.querySelectorAll<HTMLElement>('[role="tab"]')].filter(
    (t) => t.dataset.viewId !== draggedId,
  );
  for (let i = 0; i < tabs.length; i++) {
    const r = tabs[i].getBoundingClientRect();
    if (clientX < r.left + r.width / 2) return i;
  }
  return tabs.length;
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

  const resolved: Entry<View>[] = [];
  for (const id of state.zones[zone]) {
    const v = views.get(id);
    if (!state.hidden.has(id) && v) resolved.push({ ...v, id });
  }
  const activeId = state.active[zone];

  const startDrag = useCallback(
    (e: React.PointerEvent, viewId: string) => {
      e.preventDefault();
      const originIndex = state.zones[zone].indexOf(viewId);
      drag = {
        viewId, startX: e.clientX, startY: e.clientY, active: false,
        originZone: zone, originIndex, lastZone: null, lastIndex: -1,
      };

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
        if (target) {
          const idx = hitIndex(target, ev.clientX, drag.viewId);
          if (target !== drag.lastZone || idx !== drag.lastIndex) {
            drag.lastZone = target;
            drag.lastIndex = idx;
            dispatch({ type: "MOVE_VIEW", viewId: drag.viewId, toZone: target, index: idx });
          }
        }
      };

      const onUp = () => {
        const wasDrag = drag?.active;
        cleanup();
        if (!wasDrag) {
          dispatch({ type: "SET_ACTIVE", zone, viewId });
        }
      };

      const onKey = (ev: KeyboardEvent) => {
        if (ev.key !== "Escape" || !drag) return;
        const { originZone, originIndex, viewId: vid } = drag;
        cleanup();
        dispatch({ type: "MOVE_VIEW", viewId: vid, toZone: originZone, index: originIndex });
      };

      document.addEventListener("pointermove", onMove);
      document.addEventListener("pointerup", onUp);
      document.addEventListener("keydown", onKey);
    },
    [zone, state, dispatch],
  );

  if (resolved.length === 0) {
    return <div ref={ref} className="h-full" />;
  }

  if (resolved.length === 1) {
    const view = resolved[0];
    return (
      <div ref={ref} className="flex h-full flex-col">
        <Suspense
          fallback={
            <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
              Loading...
            </div>
          }
        >
          <view.component />
        </Suspense>
      </div>
    );
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
                data-view-id={view.id}
                onPointerDown={(e) => startDrag(e, view.id)}
              >
                {view.title}
                {view.closeable && (
                  <span
                    className="ml-1.5 rounded p-0.5 opacity-0 transition-opacity hover:bg-muted group-hover:opacity-100 group-data-[state=active]:opacity-60"
                    onPointerDown={(e) => e.stopPropagation()}
                    onClick={async (e) => {
                      e.stopPropagation();
                      if (await ask("Close this conversation?", { title: "Close", kind: "warning", okLabel: "Close", cancelLabel: "Cancel" }))
                        view.onClose?.();
                    }}
                  >
                    <X className="h-3 w-3" />
                  </span>
                )}
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
