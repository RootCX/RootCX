import { useSyncExternalStore } from "react";
import { subscribe, getSnapshot, subscribeCursor, getCursorSnapshot, getFocusedFile } from "./store";
import { getLanguageName } from "./languages";

export function CursorStatus() {
  const info = useSyncExternalStore(subscribeCursor, getCursorSnapshot);
  return <span className="text-xs text-muted-foreground">{info}</span>;
}

export function LanguageStatus() {
  useSyncExternalStore(subscribe, getSnapshot);
  const file = getFocusedFile();
  if (!file) return null;
  return <span className="text-xs text-muted-foreground">{getLanguageName(file.name)}</span>;
}
