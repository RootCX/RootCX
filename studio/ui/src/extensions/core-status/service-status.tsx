import { useEffect, useState } from "react";
import { useOsStatus } from "@/hooks/useOsStatus";
import type { ServiceState } from "@/types";
import { stateColor } from "@/lib/state-color";
import { getCoreUrl } from "@/core/auth";
import { Tooltip, TooltipTrigger, TooltipContent, TooltipProvider } from "@/components/ui/tooltip";

function StatusDot({ label, state, tooltip }: { label: string; state: ServiceState; tooltip?: string }) {
  const dot = (
    <div className="flex items-center gap-1.5 px-2 cursor-default">
      <span className={`h-2 w-2 rounded-full ${stateColor(state)}`} />
      <span className="text-xs text-muted-foreground">{label}</span>
    </div>
  );
  if (!tooltip) return dot;
  return (
    <Tooltip>
      <TooltipTrigger asChild>{dot}</TooltipTrigger>
      <TooltipContent side="top">{tooltip}</TooltipContent>
    </Tooltip>
  );
}

export function ServiceStatus() {
  const { status, loading } = useOsStatus();
  const [coreUrl, setCoreUrl] = useState("");

  useEffect(() => { getCoreUrl().then(setCoreUrl); }, []);

  if (loading) return <span className="text-xs text-muted-foreground">Connecting...</span>;
  if (!status) return <span className="text-xs text-muted-foreground">Disconnected</span>;

  return (
    <TooltipProvider delayDuration={300}>
      <StatusDot label="Core" state={status.runtime.state} tooltip={coreUrl || undefined} />
      <StatusDot label="Postgres" state={status.postgres.state}
        tooltip={status.postgres.port ? `localhost:${status.postgres.port}` : undefined} />
    </TooltipProvider>
  );
}
