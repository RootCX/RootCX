import { useState, useEffect } from "react";
import { RefreshCw, Plus, Trash2, Eye, EyeOff } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { ListRow } from "@/components/ui/list-row";
import { Dialog, DialogContent, DialogHeader, DialogFooter, DialogTitle, DialogDescription } from "@/components/ui/dialog";
import { listSecrets, setSecret, deleteSecret } from "@/core/api";

const errMsg = (e: unknown) => (e instanceof Error ? e.message : String(e));

function AddForm({ onDone }: { onDone: () => void }) {
  const [key, setKey] = useState("");
  const [value, setValue] = useState("");
  const [show, setShow] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const valid = key.trim() && value.trim();

  const handleSubmit = async () => {
    if (!valid) return;
    setSaving(true); setError(null);
    try { await setSecret(key.trim(), value.trim()); onDone(); }
    catch (e) { setError(errMsg(e)); }
    finally { setSaving(false); }
  };

  return (
    <div className="flex flex-col gap-2 rounded-md border border-border p-3">
      <Input size="xs" className="font-mono" placeholder="SECRET_KEY"
        value={key} onChange={(e) => setKey(e.target.value.replace(/[^A-Za-z0-9_]/g, "").toUpperCase())} />
      <div className="relative">
        <Input size="xs" className="font-mono pr-6" type={show ? "text" : "password"}
          placeholder="value" value={value} onChange={(e) => setValue(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && handleSubmit()} />
        <button type="button" tabIndex={-1} onClick={() => setShow(!show)}
          className="absolute right-1.5 top-1/2 -translate-y-1/2 text-muted-foreground hover:text-foreground">
          {show ? <EyeOff className="h-3 w-3" /> : <Eye className="h-3 w-3" />}
        </button>
      </div>
      <div className="flex items-center gap-2">
        <Button size="xs" onClick={handleSubmit} disabled={!valid || saving}>{saving ? "..." : "Add"}</Button>
        <button className="text-[10px] text-muted-foreground hover:text-foreground" onClick={onDone}>Cancel</button>
      </div>
      {error && <p className="text-[10px] text-red-400">{error}</p>}
    </div>
  );
}

export default function SecretsPanel() {
  const [keys, setKeys] = useState<string[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [showAdd, setShowAdd] = useState(false);
  const [deleting, setDeleting] = useState<string | null>(null);

  const load = () => {
    setLoading(true);
    listSecrets().then(setKeys).catch((e) => setError(errMsg(e))).finally(() => setLoading(false));
  };
  useEffect(load, []);

  const handleDelete = async (key: string) => {
    setDeleting(null); setError(null);
    try { await deleteSecret(key); load(); }
    catch (e) { setError(errMsg(e)); }
  };

  return (
    <div className="flex h-full flex-col">
      <div className="flex shrink-0 items-center justify-between border-b border-border px-3 py-1.5">
        <span className="text-xs font-medium uppercase tracking-wider text-muted-foreground">Platform Secrets</span>
        <div className="flex items-center gap-1">
          {!showAdd && (
            <Button size="xs" variant="outline" onClick={() => setShowAdd(true)}>
              <Plus className="h-3 w-3" /> Add
            </Button>
          )}
          <Button size="icon-xs" variant="ghost" onClick={load}>
            <RefreshCw className={loading ? "h-3.5 w-3.5 animate-spin" : "h-3.5 w-3.5"} />
          </Button>
        </div>
      </div>

      <div className="flex flex-1 flex-col gap-2 overflow-auto p-3">
        {showAdd && <AddForm onDone={() => { setShowAdd(false); load(); }} />}

        {keys.map((key) => (
          <ListRow key={key} className="px-3 py-1.5">
            <span className="flex-1 truncate font-mono text-xs">{key}</span>
            <span className="text-[10px] text-muted-foreground">encrypted</span>
            <Button size="icon-xs" variant="ghost" className="text-muted-foreground hover:text-red-400"
              onClick={() => setDeleting(key)}>
              <Trash2 className="h-3 w-3" />
            </Button>
          </ListRow>
        ))}

        {loading && keys.length === 0 && (
          <p className="animate-pulse py-6 text-center text-xs text-muted-foreground">Loading...</p>
        )}
        {!loading && keys.length === 0 && !showAdd && (
          <p className="py-8 text-center text-xs text-muted-foreground">No secrets configured</p>
        )}
        {error && <p className="text-[10px] text-red-400">{error}</p>}
      </div>

      <Dialog open={!!deleting} onOpenChange={(open) => !open && setDeleting(null)}>
        <DialogContent className="max-w-xs">
          <DialogHeader>
            <DialogTitle>Delete secret</DialogTitle>
            <DialogDescription>
              Remove <strong className="font-mono">{deleting}</strong>? Services using this secret will lose access.
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button size="sm" variant="ghost" onClick={() => setDeleting(null)}>Cancel</Button>
            <Button size="sm" variant="destructive" onClick={() => handleDelete(deleting!)}>Delete</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
