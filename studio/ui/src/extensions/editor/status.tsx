import { useSyncExternalStore } from "react";
import { subscribe, getSnapshot, subscribeCursor, getCursorSnapshot } from "./store";
import { getLanguageName } from "./languages";

export function CursorStatus() {
  const info = useSyncExternalStore(subscribeCursor, getCursorSnapshot);
  return <span className="text-xs text-muted-foreground">{info}</span>;
}

export function LanguageStatus() {
  const { activeTab, tabs } = useSyncExternalStore(subscribe, getSnapshot);
  const tab = tabs.find((t) => t.path === activeTab);
  if (!tab) return null;
  return (
    <span className="text-xs text-muted-foreground">
      {getLanguageName(tab.name)}
    </span>
  );
}
