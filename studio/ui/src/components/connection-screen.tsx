import { useState, useEffect } from "react";
import { ArrowLeft } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Logo } from "@/components/logo";
import { connectTo, getCoreUrl, getSavedConnections } from "@/core/auth";

const INPUT = "flex-1 rounded-md border border-border bg-background px-3 py-1.5 text-sm text-foreground outline-none focus:border-primary font-mono";

export function ConnectionScreen() {
  const [url, setUrl] = useState("");
  const [connecting, setConnecting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [recent] = useState(getSavedConnections);
  const [lastUrl, setLastUrl] = useState("");

  useEffect(() => { getCoreUrl().then((u) => { if (/^https?:\/\//i.test(u)) setLastUrl(u); }); }, []);

  async function handleConnect(target: string) {
    setError(null);
    setConnecting(true);
    try {
      if (!(await connectTo(target))) setError(`Cannot reach ${target}`);
    } finally {
      setConnecting(false);
    }
  }

  return (
    <div className="relative flex h-screen w-screen items-center justify-center overflow-hidden bg-background">
      <Logo className="pointer-events-none absolute h-[65%] max-h-[440px] text-white/[0.025]" />

      <div className="z-10 flex w-96 flex-col gap-4 rounded-lg border border-border bg-sidebar p-6">
        {lastUrl && (
          <button onClick={() => handleConnect(lastUrl)} disabled={connecting}
            className="flex items-center gap-1.5 text-xs text-muted-foreground transition-colors hover:text-foreground disabled:opacity-50">
            <ArrowLeft className="h-3 w-3" /> Back to {new URL(lastUrl).host}
          </button>
        )}
        <h1 className="text-center text-lg font-semibold text-foreground">Connect to RootCX</h1>

        <div className="flex flex-col gap-1.5">
          <label className="text-xs text-muted-foreground">Server URL</label>
          <div className="flex gap-1.5">
            <input
              type="url"
              autoFocus
              value={url}
              onChange={(e) => { setUrl(e.target.value); setError(null); }}
              onKeyDown={(e) => e.key === "Enter" && url.trim() && handleConnect(url.trim())}
              placeholder="https://core.company.com"
              className={INPUT}
            />
            <Button onClick={() => handleConnect(url.trim())} disabled={!url.trim() || connecting}>
              {connecting ? "…" : "Connect"}
            </Button>
          </div>
        </div>

        {error && (
          <div className="rounded-md border border-red-900/50 bg-red-950/30 px-3 py-2 text-xs text-red-400">{error}</div>
        )}

        {recent.length > 0 && (
          <div className="flex flex-col gap-1">
            <span className="text-[10px] font-medium uppercase tracking-wider text-muted-foreground">Recent</span>
            {recent.map((c) => (
              <button
                key={c.url}
                onClick={() => handleConnect(c.url)}
                disabled={connecting}
                className="rounded px-2 py-1 text-left text-xs font-mono text-muted-foreground transition-colors hover:bg-accent hover:text-foreground disabled:opacity-50"
              >
                {c.url}
              </button>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
