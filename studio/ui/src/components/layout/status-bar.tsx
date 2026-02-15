import { useOsStatus } from "@/hooks/useOsStatus";
import type { ServiceState } from "@/types";
import { stateColor } from "@/lib/state-color";
import { cn } from "@/lib/utils";

function StatusDot({ label, state }: { label: string; state: ServiceState }) {
  return (
    <div className="flex items-center gap-1.5 px-2">
      <span className={cn("h-2 w-2 rounded-full", stateColor(state))} />
      <span className="text-xs text-muted-foreground">{label}</span>
    </div>
  );
}

export function StatusBar() {
  const { status, loading } = useOsStatus();

  return (
    <div className="flex h-6 shrink-0 items-center border-t border-border bg-sidebar px-2">
      {loading ? (
        <span className="text-xs text-muted-foreground">Connecting...</span>
      ) : status ? (
        <div className="flex items-center">
          <StatusDot label="Runtime" state={status.runtime.state} />
          <StatusDot label="Postgres" state={status.postgres.state} />
          <StatusDot label="Forge" state={status.forge.state} />
        </div>
      ) : (
        <span className="text-xs text-muted-foreground">Disconnected</span>
      )}
    </div>
  );
}
