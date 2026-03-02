import { useEffect, useState, useSyncExternalStore } from "react";
import { Play, Square, RefreshCw } from "lucide-react";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader, DialogBody, DialogFooter, DialogTitle, DialogDescription } from "@/components/ui/dialog";
import { ListRow } from "@/components/ui/list-row";
import { subscribe, getSnapshot, refreshWorkers, startWorker, stopWorker } from "./store";

const STATUS_COLOR: Record<string, string> = {
  running: "bg-emerald-500",
  starting: "bg-amber-500",
  stopping: "bg-amber-500",
  stopped: "bg-muted-foreground/40",
  crashed: "bg-red-500",
};

function isAlive(status: string) {
  return status === "running" || status === "starting";
}

export default function WorkersPanel() {
  const { workers, loading, error } = useSyncExternalStore(subscribe, getSnapshot);
  const [stopping, setStopping] = useState<string | null>(null);

  useEffect(() => { refreshWorkers(); }, []);

  return (
    <div className="flex h-full flex-col">
      <div className="flex shrink-0 items-center justify-between border-b border-border px-3 py-1.5">
        <span className="text-xs font-medium uppercase tracking-wider text-muted-foreground">Workers</span>
        <Button size="icon-xs" variant="ghost" onClick={() => refreshWorkers()}>
          <RefreshCw className={cn("h-3.5 w-3.5", loading && "animate-spin")} />
        </Button>
      </div>

      {error && <div className="px-3 py-2 text-xs text-red-400">{error}</div>}

      <div className="flex-1 overflow-auto">
        {workers.length === 0 && !loading && (
          <div className="flex items-center justify-center p-6 text-xs text-muted-foreground">No workers</div>
        )}
        {loading && workers.length === 0 && (
          <div className="flex animate-pulse items-center justify-center p-6 text-xs text-muted-foreground">Loading...</div>
        )}
        {workers.map((w) => {
          const alive = isAlive(w.status);
          return (
            <ListRow key={w.appId} className="px-3 py-1.5">
              <span className={cn("h-2 w-2 shrink-0 rounded-full", STATUS_COLOR[w.status] ?? "bg-muted-foreground/40")} />
              <span className="flex-1 truncate text-xs">{w.appId}</span>
              <span className="text-[10px] text-muted-foreground">{w.status}</span>
              <Button
                size="icon-xs"
                variant="ghost"
                onClick={() => alive ? setStopping(w.appId) : startWorker(w.appId)}
                title={alive ? "Stop" : "Start"}
              >
                {alive ? <Square className="h-3 w-3" /> : <Play className="h-3 w-3" />}
              </Button>
            </ListRow>
          );
        })}
      </div>

      <Dialog open={!!stopping} onOpenChange={(open) => !open && setStopping(null)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Stop worker</DialogTitle>
            <DialogDescription>This will terminate the worker process for <strong>{stopping}</strong>.</DialogDescription>
          </DialogHeader>
          <DialogBody>
            <p className="text-xs text-muted-foreground">Any in-flight requests will be dropped.</p>
          </DialogBody>
          <DialogFooter>
            <Button size="sm" variant="ghost" onClick={() => setStopping(null)}>Cancel</Button>
            <Button size="sm" variant="destructive" onClick={() => { stopWorker(stopping!); setStopping(null); }}>Stop</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
