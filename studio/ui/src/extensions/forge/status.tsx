import { useSyncExternalStore } from "react";
import { subscribe, getSnapshot, type ForgePhase } from "./store";
import { cn } from "@/lib/utils";

const PHASE_LABEL: Record<ForgePhase, string> = {
  idle: "Forge",
  analyzing: "Analyzing…",
  planning: "Planning…",
  executing: "Building…",
  verifying: "Verifying…",
  done: "Done",
  error: "Error",
  stopped: "Stopped",
};

const PHASE_DOT: Record<string, string> = {
  idle: "bg-gray-500",
  done: "bg-green-500",
  error: "bg-red-500",
  stopped: "bg-orange-500",
};

export function ForgePhaseStatus() {
  const { phase } = useSyncExternalStore(subscribe, getSnapshot);
  const dot = PHASE_DOT[phase] ?? "bg-blue-500 animate-[pulse-dot_1.5s_infinite]";
  return (
    <span className="flex items-center gap-1.5 text-xs text-muted-foreground">
      <span className={cn("h-2 w-2 rounded-full", dot)} />
      {PHASE_LABEL[phase]}
    </span>
  );
}
