import {
  createContext,
  useContext,
  useReducer,
  useEffect,
  type ReactNode,
} from "react";
import { windowLabel } from "@/core/window";

export type ZoneId = "sidebar" | "editor" | "bottom" | "right";

export interface LayoutState {
  zones: Record<ZoneId, string[]>;
  active: Record<ZoneId, string | null>;
  hidden: Set<string>;
}

export type Action =
  | { type: "MOVE_VIEW"; viewId: string; toZone: ZoneId; index?: number }
  | { type: "TOGGLE_VIEW"; viewId: string }
  | { type: "SHOW_VIEW"; viewId: string; zone?: ZoneId }
  | { type: "SET_ACTIVE"; zone: ZoneId; viewId: string }
  | { type: "RESET"; defaultState: LayoutState };

const STORAGE_KEY = `studio:layout:${windowLabel}`;
const ZONE_IDS: ZoneId[] = ["sidebar", "editor", "bottom", "right"];

const DEFAULT_VISIBLE = new Set(["forge", "explorer", "welcome", "database"]);

export function buildDefaultState(
  views: { id: string; defaultZone: ZoneId; defaultActive?: boolean }[],
): LayoutState {
  const zones = Object.fromEntries(ZONE_IDS.map((z) => [z, []])) as unknown as Record<ZoneId, string[]>;
  for (const v of views) zones[v.defaultZone].push(v.id);
  const active = Object.fromEntries(ZONE_IDS.map((z) => [z, zones[z][0] ?? null])) as Record<ZoneId, string | null>;
  const hidden = new Set<string>();
  for (const v of views) {
    if (v.defaultActive) active[v.defaultZone] = v.id;
    if (!DEFAULT_VISIBLE.has(v.id)) hidden.add(v.id);
  }
  return { zones, active, hidden };
}

function reducer(state: LayoutState, action: Action): LayoutState {
  switch (action.type) {
    case "MOVE_VIEW": {
      const zones = { ...state.zones };
      for (const z of ZONE_IDS) zones[z] = zones[z].filter((id) => id !== action.viewId);
      const target = [...zones[action.toZone]];
      target.splice(action.index ?? target.length, 0, action.viewId);
      zones[action.toZone] = target;

      const active = { ...state.active };
      for (const z of ZONE_IDS) {
        if (active[z] && !zones[z].includes(active[z]!)) active[z] = zones[z][0] ?? null;
      }
      active[action.toZone] = action.viewId;

      const hidden = new Set(state.hidden);
      hidden.delete(action.viewId);
      return { zones, active, hidden };
    }

    case "TOGGLE_VIEW": {
      const hidden = new Set(state.hidden);
      if (hidden.has(action.viewId)) {
        hidden.delete(action.viewId);
        const active = { ...state.active };
        for (const z of ZONE_IDS) {
          if (state.zones[z].includes(action.viewId)) {
            active[z] = action.viewId;
            break;
          }
        }
        return { ...state, active, hidden };
      }
      hidden.add(action.viewId);
      return { ...state, hidden };
    }

    case "SHOW_VIEW": {
      const hidden = new Set(state.hidden);
      hidden.delete(action.viewId);
      const active = { ...state.active };
      const existing = ZONE_IDS.find((z) => state.zones[z].includes(action.viewId));
      if (existing) {
        active[existing] = action.viewId;
        return { ...state, active, hidden };
      }
      if (!action.zone) return { ...state, active, hidden };
      const zones = { ...state.zones, [action.zone]: [...state.zones[action.zone], action.viewId] };
      active[action.zone] = action.viewId;
      return { ...state, zones, active, hidden };
    }

    case "SET_ACTIVE":
      return { ...state, active: { ...state.active, [action.zone]: action.viewId } };

    case "RESET":
      return action.defaultState;
  }
}

function serialize(s: LayoutState): string {
  return JSON.stringify({ zones: s.zones, active: s.active, hidden: [...s.hidden] });
}

function deserialize(json: string, validIds: Set<string>): LayoutState | null {
  try {
    const p = JSON.parse(json);
    const zones = {} as Record<ZoneId, string[]>;
    const active = {} as Record<ZoneId, string | null>;
    for (const z of ZONE_IDS) {
      zones[z] = (p.zones[z] as string[]).filter((id) => validIds.has(id));
      active[z] = validIds.has(p.active[z]) ? p.active[z] : zones[z][0] ?? null;
    }
    return { zones, active, hidden: new Set((p.hidden as string[]).filter((id) => validIds.has(id))) };
  } catch {
    return null;
  }
}

const Ctx = createContext<{ state: LayoutState; dispatch: React.Dispatch<Action> } | null>(null);

export function LayoutProvider({
  defaultState,
  validIds,
  children,
}: {
  defaultState: LayoutState;
  validIds: Set<string>;
  children: ReactNode;
}) {
  const stored = localStorage.getItem(STORAGE_KEY);
  const initial = (stored && deserialize(stored, validIds)) || defaultState;
  const [state, dispatch] = useReducer(reducer, initial);

  useEffect(() => {
    localStorage.setItem(STORAGE_KEY, serialize(state));
  }, [state]);

  return <Ctx.Provider value={{ state, dispatch }}>{children}</Ctx.Provider>;
}

export function useLayout() {
  const ctx = useContext(Ctx);
  if (!ctx) throw new Error("useLayout requires LayoutProvider");
  return ctx;
}
