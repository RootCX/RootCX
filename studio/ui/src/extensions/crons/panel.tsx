import { useEffect, useState } from "react";
import { RefreshCw, ChevronRight, CheckCircle2, XCircle, Loader2, Clock } from "lucide-react";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { listApps, listAllCrons, listCronRuns, type AppSummary, type CronSchedule, type CronRun } from "@/core/api";
import { humanizeCron, formatDuration } from "./humanize";

const errMsg = (e: unknown) => (e instanceof Error ? e.message : String(e));

export default function CronsPanel() {
  const [apps, setApps] = useState<AppSummary[]>([]);
  const [crons, setCrons] = useState<CronSchedule[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [runs, setRuns] = useState<Record<string, CronRun[] | null>>({});

  const load = () => {
    let stale = false;
    setLoading(true);
    setError(null);
    Promise.all([listApps(), listAllCrons()])
      .then(([a, c]) => {
        if (stale) return;
        setApps(a);
        setCrons(c);
      })
      .catch((e) => {
        if (!stale) setError(errMsg(e));
      })
      .finally(() => {
        if (!stale) setLoading(false);
      });
    return () => { stale = true; };
  };

  useEffect(load, []);

  const toggleExpand = (cron: CronSchedule) => {
    const next = expanded === cron.id ? null : cron.id;
    setExpanded(next);
    if (next && runs[cron.id] === undefined) {
      setRuns((prev) => ({ ...prev, [cron.id]: null }));
      listCronRuns(cron.appId, cron.id)
        .then((r) => setRuns((prev) => ({ ...prev, [cron.id]: r })))
        .catch(() => setRuns((prev) => ({ ...prev, [cron.id]: [] })));
    }
  };

  const byApp = new Map<string, CronSchedule[]>();
  for (const c of crons) {
    if (!byApp.has(c.appId)) byApp.set(c.appId, []);
    byApp.get(c.appId)!.push(c);
  }

  return (
    <div className="flex h-full flex-col">
      <div className="flex shrink-0 items-center justify-between border-b border-border px-3 py-1.5">
        <span className="text-xs font-medium uppercase tracking-wider text-muted-foreground">
          Scheduled Jobs
        </span>
        <Button size="icon-xs" variant="ghost" onClick={load}>
          <RefreshCw className={cn("h-3.5 w-3.5", loading && "animate-spin")} />
        </Button>
      </div>

      {error && <div className="px-3 py-2 text-xs text-red-400">{error}</div>}

      <div className="flex-1 overflow-auto">
        {loading && crons.length === 0 && (
          <div className="flex animate-pulse items-center justify-center p-6 text-xs text-muted-foreground">
            Loading...
          </div>
        )}
        {!loading && crons.length === 0 && !error && (
          <div className="flex flex-col items-center justify-center gap-2 p-8 text-center">
            <Clock className="h-5 w-5 text-muted-foreground/50" />
            <p className="text-xs text-muted-foreground">No scheduled jobs.</p>
            <p className="text-[10px] text-muted-foreground/70">Apps declare crons in their manifest.</p>
          </div>
        )}

        {Array.from(byApp.entries()).map(([appId, appCrons]) => (
          <div key={appId} className="border-b border-border/60">
            <div className="flex items-center justify-between px-3 py-1 bg-muted/30">
              <span className="text-[10px] font-medium uppercase tracking-wider text-muted-foreground/80">
                {apps.find((a) => a.id === appId)?.name ?? appId}
              </span>
              <span className="text-[10px] text-muted-foreground/60">
                {appCrons.length} {appCrons.length === 1 ? "job" : "jobs"}
              </span>
            </div>
            {appCrons.map((cron) => (
              <div key={cron.id}>
                <button
                  onClick={() => toggleExpand(cron)}
                  className="flex w-full items-center gap-2 px-3 py-1.5 text-left hover:bg-accent/30 transition-colors"
                >
                  <ChevronRight
                    className={cn(
                      "h-3 w-3 text-muted-foreground transition-transform shrink-0",
                      expanded === cron.id && "rotate-90",
                    )}
                  />
                  <span
                    className={cn(
                      "h-1.5 w-1.5 shrink-0 rounded-full",
                      cron.enabled ? "bg-emerald-500" : "bg-muted-foreground/40",
                    )}
                    title={cron.enabled ? "enabled" : "disabled"}
                  />
                  <div className="min-w-0 flex-1">
                    <p className="truncate text-xs text-foreground">{cron.name}</p>
                    <p
                      className="truncate text-[10px] text-muted-foreground"
                      title={cron.schedule}
                    >
                      {humanizeCron(cron.schedule)}
                      {cron.timezone ? ` · ${cron.timezone}` : ""}
                    </p>
                  </div>
                </button>
                {expanded === cron.id && (
                  <div className="border-l-2 border-border bg-muted/20 px-3 py-2 ml-4">
                    <RunHistory runs={runs[cron.id]} />
                  </div>
                )}
              </div>
            ))}
          </div>
        ))}
      </div>
    </div>
  );
}

function RunHistory({ runs }: { runs: CronRun[] | null | undefined }) {
  if (runs === undefined || runs === null) {
    return <p className="text-[10px] text-muted-foreground">Loading run history…</p>;
  }
  if (runs.length === 0) {
    return <p className="text-[10px] text-muted-foreground">No runs recorded yet.</p>;
  }
  return (
    <div className="flex flex-col gap-1">
      {runs.slice(0, 10).map((r) => (
        <div key={r.runid} className="flex items-center gap-2 text-[10px]">
          <StatusIcon status={r.status} />
          <span className="text-muted-foreground shrink-0 font-mono">
            {r.startTime ? new Date(r.startTime).toLocaleString() : "—"}
          </span>
          <span className="text-muted-foreground/60 shrink-0 font-mono">
            {formatDuration(r.startTime, r.endTime)}
          </span>
          {r.returnMessage && (
            <span className="truncate text-muted-foreground/80 font-mono" title={r.returnMessage}>
              {r.returnMessage}
            </span>
          )}
        </div>
      ))}
      {runs.length > 10 && (
        <p className="text-[9px] text-muted-foreground/60 pt-1">+{runs.length - 10} older runs</p>
      )}
    </div>
  );
}

function StatusIcon({ status }: { status: string }) {
  if (status === "succeeded") return <CheckCircle2 className="h-2.5 w-2.5 shrink-0 text-emerald-500" />;
  if (status === "failed") return <XCircle className="h-2.5 w-2.5 shrink-0 text-red-500" />;
  if (status === "running" || status === "starting")
    return <Loader2 className="h-2.5 w-2.5 shrink-0 animate-spin text-amber-500" />;
  return <span className="h-2.5 w-2.5 shrink-0" />;
}
