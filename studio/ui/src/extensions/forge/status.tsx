import { useSyncExternalStore } from "react";
import { subscribe, getSnapshot } from "./store";
import { cn } from "@/lib/utils";

export function ForgeStatus() {
  const { connected, streaming } = useSyncExternalStore(subscribe, getSnapshot);
  const label = streaming ? "Working..." : connected ? "AI Forge" : "Disconnected";
  const dot = streaming
    ? "bg-blue-500 animate-[pulse-dot_1.5s_infinite]"
    : connected
      ? "bg-green-500"
      : "bg-gray-500";

  return (
    <span className="flex items-center gap-1.5 text-xs text-muted-foreground">
      <span className={cn("h-2 w-2 rounded-full", dot)} />
      {label}
    </span>
  );
}
