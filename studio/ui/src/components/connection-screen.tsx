import { useState } from "react";
import { Container, ExternalLink, Globe, Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Logo } from "@/components/logo";
import { connectTo, getSavedConnections } from "@/core/auth";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-shell";

type Step = "choose" | "local" | "remote";

const INPUT = "flex-1 rounded-md border border-border bg-background px-3 py-1.5 text-sm text-foreground outline-none focus:border-primary font-mono";
const CARD = "flex cursor-pointer items-center gap-4 rounded-lg border border-border bg-sidebar p-5 transition-colors hover:border-primary/50 hover:bg-accent/30";

const LOCAL_URL = "http://localhost:9100";

function ChooseStep({ onPick }: { onPick: (s: Step) => void }) {
  const recent = getSavedConnections();

  return (
    <div className="flex w-96 flex-col gap-6">
      <div className="flex flex-col items-center gap-2">
        <Logo className="h-8 text-primary" />
        <h1 className="text-xl font-semibold text-foreground">Welcome to RootCX</h1>
        <p className="text-sm text-muted-foreground">How would you like to get started?</p>
      </div>

      <div className="flex flex-col gap-3">
        <button className={CARD} onClick={() => onPick("local")}>
          <Container className="h-6 w-6 shrink-0 text-primary" />
          <div className="flex flex-col text-left">
            <span className="text-sm font-medium text-foreground">Run locally</span>
            <span className="text-xs text-muted-foreground">Start a RootCX Core instance on this machine via Docker</span>
          </div>
        </button>

        <button className={CARD} onClick={() => onPick("remote")}>
          <Globe className="h-6 w-6 shrink-0 text-primary" />
          <div className="flex flex-col text-left">
            <span className="text-sm font-medium text-foreground">Connect to a server</span>
            <span className="text-xs text-muted-foreground">Use a remote or cloud-hosted RootCX Core</span>
          </div>
        </button>
      </div>

      {recent.length > 0 && (
        <div className="flex flex-col gap-1 rounded-lg border border-border bg-sidebar p-4">
          <span className="text-[10px] font-medium uppercase tracking-wider text-muted-foreground">Recent</span>
          {recent.map((c) => (
            <RecentButton key={c.url} url={c.url} />
          ))}
        </div>
      )}
    </div>
  );
}

function RecentButton({ url }: { url: string }) {
  const [connecting, setConnecting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function handleClick() {
    setConnecting(true);
    setError(null);
    if (!(await connectTo(url))) setError("Unreachable");
    setConnecting(false);
  }

  return (
    <button onClick={handleClick} disabled={connecting}
      className="flex items-center justify-between rounded px-2 py-1 text-left text-xs font-mono text-muted-foreground transition-colors hover:bg-accent hover:text-foreground disabled:opacity-50">
      <span className="truncate">{url}</span>
      {connecting && <Loader2 className="h-3 w-3 animate-spin" />}
      {error && <span className="text-red-400 text-[10px]">{error}</span>}
    </button>
  );
}

function LocalStep({ onBack }: { onBack: () => void }) {
  const [status, setStatus] = useState<"idle" | "checking" | "no-docker" | "starting" | "error">("idle");
  const [error, setError] = useState<string | null>(null);

  async function handleStart() {
    setError(null);

    // Already running?
    setStatus("checking");
    if (await connectTo(LOCAL_URL)) return;

    // Docker available?
    try {
      const hasDocker = await invoke<boolean>("check_docker");
      if (!hasDocker) { setStatus("no-docker"); return; }
    } catch { setStatus("no-docker"); return; }

    setStatus("starting");
    try {
      await invoke("start_local_core");
      if (!(await connectTo(LOCAL_URL))) {
        setError("Core started but connection failed. Check Docker logs.");
        setStatus("error");
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setStatus("error");
    }
  }

  return (
    <div className="flex w-96 flex-col gap-5">
      <div className="flex flex-col items-center gap-2">
        <Logo className="h-8 text-primary" />
        <h1 className="text-xl font-semibold text-foreground">Run locally</h1>
        <p className="text-sm text-muted-foreground">
          RootCX Core runs as a Docker container on your machine.
        </p>
      </div>

      <div className="flex flex-col gap-3 rounded-lg border border-border bg-sidebar p-5">
        {status === "no-docker" ? (
          <>
            <p className="text-sm text-foreground">Docker is required but not running.</p>
            <p className="text-xs text-muted-foreground">
              Install Docker Desktop from{" "}
              <button onClick={() => open("https://docker.com/get-started")} className="text-primary hover:underline cursor-pointer">
                docker.com
              </button>
              , then try again.
            </p>
            <Button onClick={handleStart} className="cursor-pointer">Retry</Button>
          </>
        ) : (
          <>
            <Button onClick={handleStart} disabled={status === "checking" || status === "starting"} className="cursor-pointer">
              {status === "checking" && <><Loader2 className="mr-2 h-4 w-4 animate-spin" />Checking…</>}
              {status === "starting" && <><Loader2 className="mr-2 h-4 w-4 animate-spin" />Starting Core…</>}
              {(status === "idle" || status === "error") && "Start RootCX Core"}
            </Button>
            <p className="text-[11px] text-center text-muted-foreground">
              Requires Docker Desktop running on this machine
            </p>
          </>
        )}
        {error && <p className="text-xs text-red-400">{error}</p>}
      </div>

      <button onClick={onBack} className="text-xs text-muted-foreground hover:text-foreground cursor-pointer">
        Back
      </button>
    </div>
  );
}

function RemoteStep({ onBack }: { onBack: () => void }) {
  const [url, setUrl] = useState("");
  const [connecting, setConnecting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function handleConnect() {
    setError(null);
    setConnecting(true);
    if (!(await connectTo(url.trim()))) setError("Can't reach that server. Check the URL and try again.");
    setConnecting(false);
  }

  return (
    <div className="flex w-96 flex-col gap-5">
      <div className="flex flex-col items-center gap-2">
        <Logo className="h-8 text-primary" />
        <h1 className="text-xl font-semibold text-foreground">Connect to a server</h1>
        <p className="text-sm text-muted-foreground">
          Enter the URL of your RootCX Core instance.{" "}
          <button onClick={() => open("https://rootcx.com")}
            className="inline-flex items-center gap-0.5 text-primary hover:underline cursor-pointer">
            Get a free cloud account <ExternalLink className="h-3 w-3" />
          </button>
        </p>
      </div>

      <div className="flex flex-col gap-3 rounded-lg border border-border bg-sidebar p-5">
        <div className="flex gap-1.5">
          <input type="url" autoFocus value={url}
            onChange={(e) => { setUrl(e.target.value); setError(null); }}
            onKeyDown={(e) => e.key === "Enter" && url.trim() && handleConnect()}
            placeholder="https://my-project.rootcx.com"
            className={INPUT} />
          <Button onClick={handleConnect} disabled={!url.trim() || connecting} className="cursor-pointer">
            {connecting ? "…" : "Connect"}
          </Button>
        </div>
        {error && <p className="text-xs text-red-400">{error}</p>}
      </div>

      <button onClick={onBack} className="text-xs text-muted-foreground hover:text-foreground cursor-pointer">
        Back
      </button>
    </div>
  );
}

export function ConnectionScreen() {
  const [step, setStep] = useState<Step>("choose");

  return (
    <div className="relative flex h-screen w-screen items-center justify-center overflow-hidden bg-background">
      <Logo className="pointer-events-none absolute h-[65%] max-h-[440px] text-white/[0.025]" />
      <div className="z-10">
        {step === "choose" && <ChooseStep onPick={setStep} />}
        {step === "local" && <LocalStep onBack={() => setStep("choose")} />}
        {step === "remote" && <RemoteStep onBack={() => setStep("choose")} />}
      </div>
    </div>
  );
}
