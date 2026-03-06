import { useState, useEffect, useSyncExternalStore } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  subscribe, getSnapshot, refresh,
  register, remove, start, stop,
  type McpServer,
} from "./store";

const heading = "text-xs font-semibold uppercase tracking-wider text-primary";
const errBox = "rounded-md border border-red-800 bg-red-950 px-2 py-1 text-[10px] text-red-300";
const errMsg = (e: unknown) => (e instanceof Error ? e.message : String(e));
const statusDot: Record<string, string> = { running: "bg-green-500", stopped: "bg-zinc-500", error: "bg-red-500" };

function ServerRow({ server }: { server: McpServer }) {
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const act = async (fn: () => Promise<void>) => {
    setBusy(true); setError(null);
    try { await fn(); } catch (e) { setError(errMsg(e)); }
    finally { setBusy(false); }
  };

  const { transport } = server.config;
  const detail = transport.type === "stdio"
    ? `${transport.command} ${transport.args?.join(" ") ?? ""}`
    : transport.url;

  return (
    <div className="flex flex-col gap-1 rounded-sm bg-accent px-2 py-1.5">
      <div className="flex items-center gap-2">
        <span className={`h-2 w-2 shrink-0 rounded-full ${statusDot[server.status] ?? "bg-zinc-500"}`} />
        <span className="flex-1 truncate font-mono text-[10px] text-foreground">{server.name}</span>
        <span className="text-[9px] text-muted-foreground">{server.status}</span>
      </div>
      {server.config.description && <span className="text-[10px] text-muted-foreground">{server.config.description}</span>}
      <span className="truncate text-[9px] font-mono text-muted-foreground">{detail}</span>
      <div className="flex items-center gap-1 pt-0.5">
        {server.status !== "running"
          ? <Button size="xs" variant="outline" disabled={busy} onClick={() => act(() => start(server.name))}>Start</Button>
          : <Button size="xs" variant="outline" disabled={busy} onClick={() => act(() => stop(server.name))}>Stop</Button>}
        {confirmDelete ? (
          <>
            <Button size="xs" variant="ghost" className="text-red-400 hover:text-red-300" disabled={busy}
              onClick={() => act(() => remove(server.name))}>confirm</Button>
            <Button size="xs" variant="ghost" onClick={() => setConfirmDelete(false)}>cancel</Button>
          </>
        ) : (
          <Button size="xs" variant="ghost" className="text-muted-foreground hover:text-red-400"
            onClick={() => setConfirmDelete(true)}>delete</Button>
        )}
      </div>
      {error && <div className={errBox}>{error}</div>}
    </div>
  );
}

function AddServerForm({ onDone }: { onDone: () => void }) {
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [transport, setTransport] = useState<"stdio" | "sse">("stdio");
  const [command, setCommand] = useState("");
  const [args, setArgs] = useState("");
  const [url, setUrl] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const valid = name.trim() && (transport === "stdio" ? command.trim() : url.trim());

  const handleSubmit = async () => {
    if (!valid) return;
    setSaving(true); setError(null);
    try {
      await register({
        name: name.trim(),
        description: description.trim() || undefined,
        transport: transport === "stdio"
          ? { type: "stdio", command: command.trim(), args: args.trim() ? args.trim().split(/\s+/) : [] }
          : { type: "sse", url: url.trim() },
      }, true);
      onDone();
    } catch (e) { setError(errMsg(e)); }
    finally { setSaving(false); }
  };

  return (
    <div className="flex flex-col gap-1.5 rounded-md border border-border p-2">
      <Input size="xs" placeholder="Server name" value={name}
        onChange={(e) => setName(e.target.value.replace(/[^a-z0-9_-]/gi, "").toLowerCase())} />
      <Input size="xs" placeholder="Description (optional)" value={description}
        onChange={(e) => setDescription(e.target.value)} />
      <div className="flex gap-1">
        <Button size="xs" variant={transport === "stdio" ? "default" : "outline"} onClick={() => setTransport("stdio")}>stdio</Button>
        <Button size="xs" variant={transport === "sse" ? "default" : "outline"} onClick={() => setTransport("sse")}>SSE</Button>
      </div>
      {transport === "stdio" ? (
        <>
          <Input size="xs" className="font-mono" placeholder="command (e.g. npx)" value={command}
            onChange={(e) => setCommand(e.target.value)} />
          <Input size="xs" className="font-mono" placeholder="args (e.g. -y @modelcontextprotocol/server-github)" value={args}
            onChange={(e) => setArgs(e.target.value)} />
        </>
      ) : (
        <Input size="xs" className="font-mono" placeholder="https://mcp.example.com/sse" value={url}
          onChange={(e) => setUrl(e.target.value)} />
      )}
      <div className="flex gap-1">
        <Button size="xs" onClick={handleSubmit} disabled={!valid || saving}>{saving ? "..." : "Add & Start"}</Button>
        <Button size="xs" variant="ghost" onClick={onDone}>Cancel</Button>
      </div>
      {error && <div className={errBox}>{error}</div>}
    </div>
  );
}

export default function McpServersPanel() {
  const { servers, loading, error } = useSyncExternalStore(subscribe, getSnapshot);
  const [showAdd, setShowAdd] = useState(false);

  useEffect(() => { refresh(); }, []);

  return (
    <div className="flex flex-col gap-3 p-3">
      <div className="flex items-center justify-between">
        <h3 className={heading}>MCP Servers</h3>
        {!showAdd && <Button size="xs" onClick={() => setShowAdd(true)}>Add</Button>}
      </div>
      <p className="text-[10px] text-muted-foreground">
        Connect external tool servers via the Model Context Protocol. Tools are available to all AI agents.
      </p>
      {showAdd && <AddServerForm onDone={() => setShowAdd(false)} />}
      {loading && servers.length === 0 && <span className="text-[10px] text-muted-foreground">Loading...</span>}
      {servers.length > 0 && (
        <div className="flex flex-col gap-1.5">
          {servers.map((s) => <ServerRow key={s.name} server={s} />)}
        </div>
      )}
      {!loading && servers.length === 0 && !showAdd && (
        <span className="text-[10px] text-muted-foreground">No MCP servers configured.</span>
      )}
      {error && <div className={errBox}>{error}</div>}
    </div>
  );
}
