import { useSyncExternalStore } from "react";
import type { Registry, Entry } from "./registry";
import { views, commands, statusBar } from "./studio";

function useRegistry<T>(r: Registry<T>): Entry<T>[] {
  return useSyncExternalStore(
    (cb) => r.subscribe(cb),
    () => r.getAll(),
  );
}

export function useViews() {
  return useRegistry(views);
}

export function useCommands() {
  return useRegistry(commands);
}

export function useStatusBarItems() {
  return useRegistry(statusBar);
}
