import { useOsStatus } from "@/hooks/useOsStatus";
import { stateColor } from "@/lib/state-color";
import { cn } from "@/lib/utils";

export default function WelcomePanel() {
  const { status, loading } = useOsStatus();

  return (
    <div className="flex h-full flex-col items-center justify-center gap-6 p-8">
      <div className="text-center">
        <h1 className="text-2xl font-semibold tracking-tight">
          RootCX Studio
        </h1>
        <p className="mt-1 text-sm text-muted-foreground">
          Operating System IDE
        </p>
      </div>

      {loading ? (
        <p className="text-sm text-muted-foreground">
          Connecting to Runtime...
        </p>
      ) : status ? (
        <div className="grid w-full max-w-xs gap-3">
          {(
            [
              ["Runtime", status.runtime.state],
              ["PostgreSQL", status.postgres.state],
              ["Forge", status.forge.state],
            ] as const
          ).map(([label, state]) => (
            <div
              key={label}
              className="flex items-center justify-between rounded-md border border-border bg-card px-4 py-3"
            >
              <span className="text-sm">{label}</span>
              <div className="flex items-center gap-2">
                <span
                  className={cn("h-2 w-2 rounded-full", stateColor(state))}
                />
                <span className="font-mono text-xs uppercase text-muted-foreground">
                  {state}
                </span>
              </div>
            </div>
          ))}
        </div>
      ) : (
        <p className="text-sm text-muted-foreground">Disconnected</p>
      )}
    </div>
  );
}
