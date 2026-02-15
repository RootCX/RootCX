import {
  createContext,
  useContext,
  useReducer,
  useEffect,
  type ReactNode,
} from "react";

export type ZoneId = "sidebar" | "editor" | "bottom";

export interface LayoutState {
  zones: Record<ZoneId, string[]>;
  active: Record<ZoneId, string | null>;
  hidden: Set<string>;
}

type Action =
  | { type: "MOVE_VIEW"; viewId: string; toZone: ZoneId; index?: number }
  | { type: "TOGGLE_VIEW"; viewId: string }
  | { type: "SET_ACTIVE"; zone: ZoneId; viewId: string }
  | { type: "RESET"; defaultState: LayoutState };

const STORAGE_KEY = "studio:layout";
const ZONE_IDS: ZoneId[] = ["sidebar", "editor", "bottom"];

export function buildDefaultState(
  views: { id: string; defaultZone: ZoneId }[],
): LayoutState {
  const zones: Record<ZoneId, string[]> = { sidebar: [], editor: [], bottom: [] };
  for (const v of views) zones[v.defaultZone].push(v.id);
  const active = {} as Record<ZoneId, string | null>;
  for (const z of ZONE_IDS) active[z] = zones[z][0] ?? null;
  return { zones, active, hidden: new Set() };
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
