import { useState, useCallback, useEffect, useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button } from "@/components/ui/button";
import { aiConfigStore } from "@/lib/ai-models";
import { showAISetupDialog } from "@/components/ai-setup-dialog";

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
          <button className="text-[10px] text-primary hover:underline" onClick={showAISetupDialog}>Change</button>
        </div>
      ) : (
        <Button size="sm" className="h-7 text-[10px]" onClick={showAISetupDialog}>Configure AI Provider</Button>
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
    invoke<string[]>("list_platform_secrets").then(setKeys).catch((e) => setError(errMsg(e)));
  }, []);

  useEffect(refresh, [refresh]);

  const handleAdd = async () => {
    const k = newKey.trim(), v = newValue.trim();
    if (!k || !v) return;
    setError(null);
    try {
      await invoke("set_platform_secret", { key: k, value: v });
      setNewKey("");
      setNewValue("");
      refresh();
    } catch (e) { setError(errMsg(e)); }
  };

  const handleDelete = async (key: string) => {
    setError(null);
    try {
      await invoke("delete_platform_secret", { key });
      setConfirmDelete(null);
      refresh();
    } catch (e) { setError(errMsg(e)); }
  };

  const inp = "h-6 rounded-md border border-input bg-background px-2 font-mono text-[10px] text-foreground placeholder:text-muted-foreground focus:border-ring focus:outline-none";

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
                  <button className="text-[10px] text-red-400 hover:text-red-300" onClick={() => handleDelete(key)}>confirm</button>
                  <button className="text-[10px] text-muted-foreground hover:text-foreground" onClick={() => setConfirmDelete(null)}>cancel</button>
                </div>
              ) : (
                <button className="text-[10px] text-muted-foreground hover:text-red-400" onClick={() => setConfirmDelete(key)}>delete</button>
              )}
            </div>
          ))}
        </div>
      ) : (
        <span className="text-[10px] text-muted-foreground">No secrets configured.</span>
      )}

      <div className="flex gap-1.5">
        <input type="text" className={`${inp} w-2/5`} placeholder="KEY" value={newKey}
          onChange={(e) => setNewKey(e.target.value.replace(/[^A-Za-z0-9_]/g, "").toUpperCase())} />
        <input type="password" className={`${inp} flex-1`} placeholder="value" value={newValue}
          onChange={(e) => setNewValue(e.target.value)} onKeyDown={(e) => e.key === "Enter" && handleAdd()} />
        <Button size="sm" className="h-6 px-2 text-[10px]" onClick={handleAdd} disabled={!newKey.trim() || !newValue.trim()}>Add</Button>
      </div>

      {error && <div className="rounded-md border border-red-800 bg-red-950 px-2 py-1 text-[10px] text-red-300">{error}</div>}
    </div>
  );
}
