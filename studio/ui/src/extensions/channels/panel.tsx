import { useState, useEffect, useSyncExternalStore } from "react";
import { Plus, Trash2 } from "lucide-react";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { subscribe, getSnapshot, refresh, setup, remove, type Channel } from "./store";

const errMsg = (e: unknown) => (e instanceof Error ? e.message : String(e));
const statusColor = { active: "bg-green-500", inactive: "bg-zinc-500", error: "bg-red-500" } as const;

type Provider = { id: string; label: string; fields: { key: string; placeholder: string; secret?: boolean }[] };

const PROVIDERS: Provider[] = [
  { id: "telegram", label: "Telegram", fields: [
    { key: "bot_token", placeholder: "Bot token from @BotFather", secret: true },
  ]},
  { id: "slack", label: "Slack", fields: [
    { key: "bot_token", placeholder: "xoxb-... (Bot User OAuth Token)", secret: true },
    { key: "signing_secret", placeholder: "Signing Secret (Basic Information)", secret: true },
  ]},
];

function ChannelCard({ channel }: { channel: Channel }) {
  const [busy, setBusy] = useState(false);
  const [confirm, setConfirm] = useState(false);

  return (
    <div className="flex items-center gap-2.5 rounded-md border border-border px-3 py-2">
      <span className={cn("h-2 w-2 shrink-0 rounded-full", statusColor[channel.status] ?? "bg-zinc-500")} />
      <span className="flex-1 truncate text-xs font-medium capitalize">{channel.provider}</span>
      {confirm ? (
        <div className="flex items-center gap-1">
          <span className="text-[10px] text-red-400">Delete?</span>
          <Button size="xs" variant="ghost" className="text-red-400" disabled={busy}
            onClick={async () => { setBusy(true); await remove(channel.id).catch(() => {}); setBusy(false); }}>Yes</Button>
          <Button size="xs" variant="ghost" onClick={() => setConfirm(false)}>No</Button>
        </div>
      ) : (
        <Button size="xs" variant="ghost" className="text-muted-foreground hover:text-red-400"
          onClick={() => setConfirm(true)}><Trash2 className="h-3 w-3" /></Button>
      )}
    </div>
  );
}

function AddChannelForm({ onDone }: { onDone: () => void }) {
  const [provider, setProvider] = useState<Provider | null>(null);
  const [config, setConfig] = useState<Record<string, string>>({});
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const valid = provider && provider.fields.every(f => config[f.key]?.trim());

  const handleSubmit = async () => {
    if (!valid || !provider) return;
    setSaving(true); setError(null);
    try { await setup(provider.id, config); onDone(); }
    catch (e) { setError(errMsg(e)); }
    finally { setSaving(false); }
  };

  return (
    <div className="flex flex-col gap-2 rounded-md border border-border p-3">
      <select className="rounded border border-border bg-transparent px-2 py-1 text-xs"
        value={provider?.id ?? ""} onChange={(e) => {
          setProvider(PROVIDERS.find(p => p.id === e.target.value) ?? null);
          setConfig({});
        }}>
        <option value="" disabled>Select provider...</option>
        {PROVIDERS.map(p => <option key={p.id} value={p.id}>{p.label}</option>)}
      </select>
      {provider && <>
        {provider.fields.map(f => (
          <Input key={f.key} size="xs" className="font-mono" placeholder={f.placeholder}
            type={f.secret ? "password" : "text"} value={config[f.key] ?? ""}
            onChange={(e) => setConfig({ ...config, [f.key]: e.target.value })} />
        ))}
        <div className="flex items-center gap-2 pt-1">
          <Button size="xs" onClick={handleSubmit} disabled={!valid || saving}>
            {saving ? "Connecting..." : "Connect"}
          </Button>
          <button className="text-[10px] text-muted-foreground hover:text-foreground" onClick={onDone}>Cancel</button>
        </div>
        {error && <p className="text-[10px] text-red-400">{error}</p>}
      </>}
    </div>
  );
}

export default function ChannelsPanel() {
  const { channels, loading, error } = useSyncExternalStore(subscribe, getSnapshot);
  const [showAdd, setShowAdd] = useState(false);
  useEffect(() => { refresh(); }, []);

  return (
    <div className="flex h-full flex-col">
      <div className="flex shrink-0 items-center justify-between border-b border-border px-3 py-2">
        <span className="text-xs font-medium uppercase tracking-wider text-muted-foreground">Channels</span>
        {!showAdd && (
          <Button size="xs" variant="outline" onClick={() => setShowAdd(true)}>
            <Plus className="h-3 w-3" /> Add
          </Button>
        )}
      </div>
      <div className="flex-1 overflow-auto p-3">
        <div className="flex flex-col gap-2">
          {showAdd && <AddChannelForm onDone={() => setShowAdd(false)} />}
          {channels.map(ch => <ChannelCard key={ch.id} channel={ch} />)}
          {loading && channels.length === 0 && <p className="py-6 text-center text-xs text-muted-foreground">Loading...</p>}
          {!loading && channels.length === 0 && !showAdd && <p className="py-8 text-center text-xs text-muted-foreground">No channels configured</p>}
          {error && <p className="text-[10px] text-red-400">{error}</p>}
        </div>
      </div>
    </div>
  );
}
