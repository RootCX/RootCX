import { useSyncExternalStore } from "react";
import type { Registry, Entry } from "./registry";
import { views, statusBar } from "./studio";

function useRegistry<T>(r: Registry<T>): Entry<T>[] {
  return useSyncExternalStore(
    (cb) => r.subscribe(cb),
    () => r.getAll(),
  );
}

export function useViews() {
  return useRegistry(views);
}

export function useStatusBarItems() {
  return useRegistry(statusBar);
}
