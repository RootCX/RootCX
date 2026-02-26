import { useSyncExternalStore } from "react";
import { subscribe, getSnapshot } from "./store";
import { cn } from "@/lib/utils";

export function ForgeStatus() {
  const { streaming } = useSyncExternalStore(subscribe, getSnapshot);
  return (
    <span className="flex items-center gap-1.5 text-xs text-muted-foreground">
      <span className={cn("h-2 w-2 rounded-full", streaming ? "bg-blue-500 animate-[pulse-dot_1.5s_infinite]" : "bg-green-500")} />
      {streaming ? "Working..." : "AI Forge"}
    </span>
  );
}
