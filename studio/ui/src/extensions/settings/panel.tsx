import { useState, useCallback, useEffect, useSyncExternalStore } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { aiConfigStore } from "@/lib/ai-models";
import { showAISetupDialog } from "@/components/ai-setup-dialog";
import { listSecrets, setSecret, deleteSecret } from "@/core/api";

const heading = "text-xs font-semibold uppercase tracking-wider text-primary";
const errMsg = (e: unknown) => (e instanceof Error ? e.message : String(e));

function AIProviderSection() {
  const aiConfig = useSyncExternalStore(aiConfigStore.subscribe, aiConfigStore.getSnapshot);
  return (
    <>
      <h3 className={heading}>AI Provider</h3>
      {aiConfig ? (
        <div className="flex items-center gap-2">
          <span className="h-2 w-2 rounded-full bg-green-500" />
          <span className="flex-1 text-[10px] text-foreground">{aiConfigStore.providerName()}</span>
          <Button size="xs" variant="link" onClick={showAISetupDialog}>Change</Button>
        </div>
      ) : (
        <Button size="xs" onClick={showAISetupDialog}>Configure AI Provider</Button>
      )}
    </>
  );
}

export default function SettingsPanel() {
  const [keys, setKeys] = useState<string[]>([]);
  const [newKey, setNewKey] = useState("");
  const [newValue, setNewValue] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);

  const refresh = useCallback(() => {
    listSecrets().then(setKeys).catch((e) => setError(errMsg(e)));
  }, []);

  useEffect(refresh, [refresh]);

  const handleAdd = async () => {
    const k = newKey.trim(), v = newValue.trim();
    if (!k || !v) return;
    setError(null);
    try {
      await setSecret(k, v);
      setNewKey("");
      setNewValue("");
      refresh();
    } catch (e) { setError(errMsg(e)); }
  };

  const handleDelete = async (key: string) => {
    setError(null);
    try {
      await deleteSecret(key);
      setConfirmDelete(null);
      refresh();
    } catch (e) { setError(errMsg(e)); }
  };

  return (
    <div className="flex flex-col gap-3 p-3">
      <AIProviderSection />
      <div className="my-1 border-t border-border" />

      <h3 className={heading}>Platform Secrets</h3>
      <p className="text-[10px] text-muted-foreground">Environment variables injected into all workers and the AI Forge.</p>

      {keys.length > 0 ? (
        <div className="flex flex-col gap-1">
          {keys.map((key) => (
            <div key={key} className="flex items-center gap-2 rounded-sm bg-accent px-2 py-1">
              <span className="flex-1 font-mono text-[10px] text-foreground">{key}</span>
              {confirmDelete === key ? (
                <div className="flex gap-1">
                  <Button size="xs" variant="ghost" className="text-red-400 hover:text-red-300" onClick={() => handleDelete(key)}>confirm</Button>
                  <Button size="xs" variant="ghost" onClick={() => setConfirmDelete(null)}>cancel</Button>
                </div>
              ) : (
                <Button size="xs" variant="ghost" className="text-muted-foreground hover:text-red-400" onClick={() => setConfirmDelete(key)}>delete</Button>
              )}
            </div>
          ))}
        </div>
      ) : (
        <span className="text-[10px] text-muted-foreground">No secrets configured.</span>
      )}

      <div className="flex gap-1.5">
        <Input size="xs" className="w-2/5 font-mono" placeholder="KEY" value={newKey}
          onChange={(e) => setNewKey(e.target.value.replace(/[^A-Za-z0-9_]/g, "").toUpperCase())} />
        <Input size="xs" type="password" className="flex-1 font-mono" placeholder="value" value={newValue}
          onChange={(e) => setNewValue(e.target.value)} onKeyDown={(e) => e.key === "Enter" && handleAdd()} />
        <Button size="xs" onClick={handleAdd} disabled={!newKey.trim() || !newValue.trim()}>Add</Button>
      </div>

      {error && <div className="rounded-md border border-red-800 bg-red-950 px-2 py-1 text-[10px] text-red-300">{error}</div>}
    </div>
  );
}
