import { useState, useEffect } from "react";
import { Package, Loader2 } from "lucide-react";
import { listen } from "@tauri-apps/api/event";
import { executeCommand } from "@/core/studio";

export function BundleButton() {
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    const subs = [listen("run-started", () => setBusy(true)), listen("run-exited", () => setBusy(false))];
    return () => { subs.forEach((s) => s.then((fn) => fn())); };
  }, []);

  const Icon = busy ? Loader2 : Package;
  return (
    <button
      className="flex cursor-pointer items-center gap-1 rounded px-1.5 text-[11px] text-muted-foreground transition-colors hover:bg-white/10 hover:text-foreground disabled:cursor-default disabled:opacity-50"
      disabled={busy}
      onClick={() => executeCommand("rootcx.bundle")}
      title="Bundle for Distribution (Cmd+Shift+B)"
    >
      <Icon className={`h-3 w-3${busy ? " animate-spin" : ""}`} />
      <span>{busy ? "Bundling..." : "Bundle"}</span>
    </button>
  );
}
