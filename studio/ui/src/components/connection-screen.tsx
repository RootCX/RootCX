import { useState } from "react";
import { ExternalLink } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Logo } from "@/components/logo";
import { connectTo, getSavedConnections } from "@/core/auth";
import { open } from "@tauri-apps/plugin-shell";

const INPUT = "flex-1 rounded-md border border-border bg-background px-3 py-1.5 text-sm text-foreground outline-none focus:border-primary font-mono";

export function ConnectionScreen() {
  const [url, setUrl] = useState("");
  const [connecting, setConnecting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [recent] = useState(getSavedConnections);

  async function handleConnect(target: string) {
    setError(null);
    setConnecting(true);
    try {
      if (!(await connectTo(target))) setError("Can't reach that server. Check the URL and try again.");
    } finally {
      setConnecting(false);
    }
  }

  return (
    <div className="relative flex h-screen w-screen items-center justify-center overflow-hidden bg-background">
      <Logo className="pointer-events-none absolute h-[65%] max-h-[440px] text-white/[0.025]" />

      <div className="z-10 flex w-96 flex-col gap-6">
        <div className="flex flex-col items-center gap-2">
          <Logo className="h-8 text-primary" />
          <h1 className="text-xl font-semibold text-foreground">
            {recent.length ? "Reconnect to RootCX" : "Welcome to RootCX"}
          </h1>
          <p className="text-center text-sm text-muted-foreground">
            Enter your server URL to get started.{" "}
            <button
              onClick={() => open("https://rootcx.com")}
              className="inline-flex items-center gap-0.5 text-primary hover:underline cursor-pointer"
            >
              Get a free account <ExternalLink className="h-3 w-3" />
            </button>
          </p>
        </div>

        <div className="flex flex-col gap-3 rounded-lg border border-border bg-sidebar p-5">
          <div className="flex gap-1.5">
            <input
              type="url"
              autoFocus
              value={url}
              onChange={(e) => { setUrl(e.target.value); setError(null); }}
              onKeyDown={(e) => e.key === "Enter" && url.trim() && handleConnect(url.trim())}
              placeholder="https://my-project.rootcx.com"
              className={INPUT}
            />
            <Button onClick={() => handleConnect(url.trim())} disabled={!url.trim() || connecting}>
              {connecting ? "…" : "Connect"}
            </Button>
          </div>

          {error && (
            <p className="text-xs text-red-400">{error}</p>
          )}

          {recent.length > 0 && (
            <div className="flex flex-col gap-1 border-t border-border pt-3">
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
    </div>
  );
}
