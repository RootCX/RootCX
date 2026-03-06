import { useState, useEffect, useSyncExternalStore } from "react";
import { Plus, Trash2, Play, Square, ChevronDown, ChevronRight, Eye, EyeOff } from "lucide-react";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { subscribe, getSnapshot, refresh, register, remove, start, stop, type McpServer } from "./store";
import { setSecret } from "@/core/api";

const errMsg = (e: unknown) => (e instanceof Error ? e.message : String(e));
const statusColor = { running: "bg-green-500", error: "bg-red-500", stopped: "bg-zinc-500" } as const;

function ServerCard({ server }: { server: McpServer }) {
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [confirm, setConfirm] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const act = async (fn: () => Promise<void>) => {
    setBusy(true); setError(null);
    try { await fn(); } catch (e) { setError(errMsg(e)); }
    finally { setBusy(false); }
  };

  const running = server.status === "running";
  const { transport } = server.config;
  const cmd = transport.type === "stdio"
    ? `${transport.command} ${transport.args?.join(" ") ?? ""}`.trim()
    : transport.url;

  return (
    <div className="rounded-md border border-border">
      <button onClick={() => setOpen(!open)}
        className="flex w-full items-center gap-2.5 px-3 py-2 text-left hover:bg-accent/30">
        {open ? <ChevronDown className="h-3 w-3 text-muted-foreground" /> : <ChevronRight className="h-3 w-3 text-muted-foreground" />}
        <span className={cn("h-2 w-2 shrink-0 rounded-full", statusColor[server.status] ?? "bg-zinc-500")} />
        <span className="flex-1 truncate text-xs font-medium">{server.name}</span>
        <span className="text-[10px] text-muted-foreground">{server.status}</span>
      </button>
      {open && (
        <div className="flex flex-col gap-2 border-t border-border px-3 py-2">
          <p className="truncate font-mono text-[10px] text-muted-foreground">{cmd}</p>
          <div className="flex items-center gap-1.5">
            <Button size="xs" variant="outline" disabled={busy}
              onClick={() => act(() => running ? stop(server.name) : start(server.name))}>
              {running ? <><Square className="h-3 w-3" /> Stop</> : <><Play className="h-3 w-3" /> Start</>}
            </Button>
            <div className="flex-1" />
            {confirm ? (
              <div className="flex items-center gap-1">
                <span className="text-[10px] text-red-400">Delete?</span>
                <Button size="xs" variant="ghost" className="text-red-400" disabled={busy}
                  onClick={() => act(() => remove(server.name))}>Yes</Button>
                <Button size="xs" variant="ghost" onClick={() => setConfirm(false)}>No</Button>
              </div>
            ) : (
              <Button size="xs" variant="ghost" className="text-muted-foreground hover:text-red-400"
                onClick={() => setConfirm(true)}><Trash2 className="h-3 w-3" /></Button>
            )}
          </div>
          {error && <p className="text-[10px] text-red-400">{error}</p>}
        </div>
      )}
    </div>
  );
}

function PasswordInput({ value, onChange, placeholder }: {
  value: string; onChange: (v: string) => void; placeholder?: string;
}) {
  const [show, setShow] = useState(false);
  return (
    <div className="relative flex-[2]">
      <Input size="xs" className="font-mono pr-6" type={show ? "text" : "password"}
        placeholder={placeholder} value={value} onChange={(e) => onChange(e.target.value)} />
      <button type="button" tabIndex={-1} onClick={() => setShow(!show)}
        className="absolute right-1.5 top-1/2 -translate-y-1/2 text-muted-foreground hover:text-foreground">
        {show ? <EyeOff className="h-3 w-3" /> : <Eye className="h-3 w-3" />}
      </button>
    </div>
  );
}

function AddServerForm({ onDone }: { onDone: () => void }) {
  const [name, setName] = useState("");
  const [transport, setTransport] = useState<"stdio" | "sse">("stdio");
  const [command, setCommand] = useState("");
  const [args, setArgs] = useState("");
  const [url, setUrl] = useState("");
  const [envRows, setEnvRows] = useState([{ key: "", value: "" }]);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const valid = name.trim() && (transport === "stdio" ? command.trim() : url.trim());

  const updateEnv = (i: number, field: "key" | "value", v: string) =>
    setEnvRows(envRows.map((r, j) => j === i ? { ...r, [field]: v } : r));

  const handleSubmit = async () => {
    if (!valid) return;
    setSaving(true); setError(null);
    try {
      const secrets = envRows.filter(r => r.key.trim() && r.value);
      for (const s of secrets) await setSecret(s.key.trim(), s.value);
      await register({
        name: name.trim(),
        transport: transport === "stdio"
          ? { type: "stdio", command: command.trim(), args: args.trim() ? args.trim().split(/\s+/) : [] }
          : { type: "sse", url: url.trim() },
      }, true);
      onDone();
    } catch (e) { setError(errMsg(e)); }
    finally { setSaving(false); }
  };

  return (
    <div className="flex flex-col gap-2 rounded-md border border-border p-3">
      <Input size="xs" className="font-mono" placeholder="Server name" value={name}
        onChange={(e) => setName(e.target.value.replace(/[^a-z0-9_-]/gi, "").toLowerCase())} />
      <div className="flex gap-1">
        <Button size="xs" variant={transport === "stdio" ? "default" : "outline"} onClick={() => setTransport("stdio")}>stdio</Button>
        <Button size="xs" variant={transport === "sse" ? "default" : "outline"} onClick={() => setTransport("sse")}>SSE</Button>
      </div>
      {transport === "stdio" ? (
        <div className="flex gap-1.5">
          <Input size="xs" className="w-1/4 font-mono" placeholder="npx" value={command}
            onChange={(e) => setCommand(e.target.value)} />
          <Input size="xs" className="flex-1 font-mono" placeholder="-y @anysite/mcp" value={args}
            onChange={(e) => setArgs(e.target.value)} />
        </div>
      ) : (
        <Input size="xs" className="font-mono" placeholder="https://mcp.example.com/sse" value={url}
          onChange={(e) => setUrl(e.target.value)} />
      )}
      {envRows.map((r, i) => (
        <div key={i} className="flex items-center gap-1.5">
          <Input size="xs" className="w-1/3 font-mono" placeholder="ENV_KEY" value={r.key}
            onChange={(e) => updateEnv(i, "key", e.target.value.replace(/[^A-Z0-9_]/gi, "").toUpperCase())} />
          <PasswordInput value={r.value} onChange={(v) => updateEnv(i, "value", v)} placeholder="secret value" />
          {envRows.length > 1 && (
            <button className="text-muted-foreground hover:text-red-400"
              onClick={() => setEnvRows(envRows.filter((_, j) => j !== i))}>
              <Trash2 className="h-3 w-3" />
            </button>
          )}
        </div>
      ))}
      <button className="self-start text-[10px] text-muted-foreground hover:text-foreground"
        onClick={() => setEnvRows([...envRows, { key: "", value: "" }])}>+ env variable</button>
      <div className="flex items-center gap-2 pt-1">
        <Button size="xs" onClick={handleSubmit} disabled={!valid || saving}>
          {saving ? "..." : "Add & Start"}
        </Button>
        <button className="text-[10px] text-muted-foreground hover:text-foreground" onClick={onDone}>Cancel</button>
      </div>
      {error && <p className="text-[10px] text-red-400">{error}</p>}
    </div>
  );
}

export default function McpServersPanel() {
  const { servers, loading, error } = useSyncExternalStore(subscribe, getSnapshot);
  const [showAdd, setShowAdd] = useState(false);
  useEffect(() => { refresh(); }, []);

  return (
    <div className="flex h-full flex-col">
      <div className="flex shrink-0 items-center justify-between border-b border-border px-3 py-2">
        <span className="text-xs font-medium uppercase tracking-wider text-muted-foreground">MCP Servers</span>
        {!showAdd && (
          <Button size="xs" variant="outline" onClick={() => setShowAdd(true)}>
            <Plus className="h-3 w-3" /> Add
          </Button>
        )}
      </div>
      <div className="flex-1 overflow-auto p-3">
        <div className="flex flex-col gap-2">
          {showAdd && <AddServerForm onDone={() => setShowAdd(false)} />}
          {servers.map((s) => <ServerCard key={s.name} server={s} />)}
          {loading && servers.length === 0 && <p className="py-6 text-center text-xs text-muted-foreground">Loading...</p>}
          {!loading && servers.length === 0 && !showAdd && <p className="py-8 text-center text-xs text-muted-foreground">No MCP servers configured</p>}
          {error && <p className="text-[10px] text-red-400">{error}</p>}
        </div>
      </div>
    </div>
  );
}
