import { useSyncExternalStore } from "react";
import { AlertTriangle, AlertCircle, CheckCircle2, Info, X } from "lucide-react";
import { subscribe, getSnapshot, dismiss, type Notification } from "@/core/notifications";

const styles: Record<Notification["type"], { icon: typeof Info; cls: string }> = {
  info: { icon: Info, cls: "bg-blue-950/60 text-blue-400" },
  success: { icon: CheckCircle2, cls: "bg-green-950/60 text-green-400" },
  warning: { icon: AlertTriangle, cls: "bg-amber-950/60 text-amber-400" },
  error: { icon: AlertCircle, cls: "bg-red-950/60 text-red-400" },
};

export function NotificationBar() {
  const notifications = useSyncExternalStore(subscribe, getSnapshot);
  if (notifications.length === 0) return null;

  return (
    <div className="shrink-0 border-b border-border/40">
      {notifications.map((n) => {
        const { icon: Icon, cls } = styles[n.type];
        return (
          <div key={n.id} className={`flex h-8 items-center gap-2 px-3 text-xs font-medium ${cls}`}>
            <Icon className="h-3.5 w-3.5 shrink-0" />
            <span className="min-w-0 flex-1 truncate text-muted-foreground">{n.message}</span>
            {n.action && (
              <button className="cursor-pointer shrink-0 rounded-md bg-white/10 px-2 py-0.5 hover:bg-white/20" onClick={n.action.run}>
                {n.action.label}
              </button>
            )}
            <button className="cursor-pointer shrink-0 text-muted-foreground/50 hover:text-muted-foreground" onClick={() => dismiss(n.id)}>
              <X className="h-3.5 w-3.5" />
            </button>
          </div>
        );
      })}
    </div>
  );
}
