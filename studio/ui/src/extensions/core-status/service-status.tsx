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

export function ServiceStatus() {
  const { status, loading } = useOsStatus();

  if (loading) return <span className="text-xs text-muted-foreground">Connecting...</span>;
  if (!status) return <span className="text-xs text-muted-foreground">Disconnected</span>;

  return (
    <>
      <StatusDot label="Runtime" state={status.runtime.state} />
      <StatusDot label="Postgres" state={status.postgres.state} />
      <StatusDot label="AI Forge" state={status.forge.state} />
    </>
  );
}
