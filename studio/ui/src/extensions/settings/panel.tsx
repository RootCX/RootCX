import { useState, useCallback, useEffect, useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { aiConfigStore } from "@/lib/ai-models";
import { showAISetupDialog } from "@/components/ai-setup-dialog";
import { listSecretScopes, listSecrets, setSecret, deleteSecret, saveAiConfig } from "@/core/api";

const heading = "text-xs font-semibold uppercase tracking-wider text-primary";
const errBox = "rounded-md border border-red-800 bg-red-950 px-2 py-1 text-[10px] text-red-300";
const errMsg = (e: unknown) => (e instanceof Error ? e.message : String(e));

function AIProviderSection() {
  const aiConfig = useSyncExternalStore(aiConfigStore.subscribe, aiConfigStore.getSnapshot);
  const [model, setModel] = useState("");
  const [saving, setSaving] = useState(false);

  useEffect(() => { if (aiConfig?.model) setModel(aiConfig.model); }, [aiConfig?.model]);

  const dirty = !!(aiConfig && model.trim() && model !== aiConfig.model);

  const saveModel = async () => {
    if (!aiConfig || !dirty) return;
    setSaving(true);
    try {
      await saveAiConfig({ ...aiConfig, model: model.trim() });
      await invoke("forge_reload_config").catch(() => {});
      await aiConfigStore.refresh();
    } finally { setSaving(false); }
  };

  return (
    <>
      <h3 className={heading}>AI Provider</h3>
      {aiConfig ? (
        <>
          <div className="flex items-center gap-2">
            <span className="h-2 w-2 rounded-full bg-green-500" />
            <span className="flex-1 text-[10px] text-foreground">{aiConfigStore.providerName()}</span>
            <Button size="xs" variant="link" onClick={showAISetupDialog}>Change</Button>
          </div>
          <label className="flex flex-col gap-1">
            <span className="text-[10px] font-medium text-muted-foreground">Model</span>
            <div className="flex gap-1.5">
              <Input size="xs" className="flex-1 font-mono" value={model}
                onChange={(e) => setModel(e.target.value)}
                onKeyDown={(e) => e.key === "Enter" && saveModel()} />
              {dirty && <Button size="xs" onClick={saveModel} disabled={saving}>{saving ? "..." : "Save"}</Button>}
            </div>
          </label>
        </>
      ) : (
        <Button size="xs" onClick={showAISetupDialog}>Configure AI Provider</Button>
      )}
    </>
  );
}

const scopeLabel = (s: string) =>
  s === "_platform" ? "Platform" : s.startsWith("_mcp.") ? `MCP: ${s.slice(5)}` : s;

function ScopeSection({ scope, onEmpty }: { scope: string; onEmpty: () => void }) {
  const [keys, setKeys] = useState<string[]>([]);
  const [newKey, setNewKey] = useState("");
  const [newValue, setNewValue] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<string | null>(null);

  const load = useCallback(() => {
    listSecrets(scope).then(setKeys).catch((e) => setError(errMsg(e)));
  }, [scope]);
  useEffect(load, [load]);

  const handleAdd = async () => {
    const k = newKey.trim(), v = newValue.trim();
    if (!k || !v) return;
    setError(null);
    try { await setSecret(k, v, scope); setNewKey(""); setNewValue(""); load(); }
    catch (e) { setError(errMsg(e)); }
  };

  const handleDelete = async (key: string) => {
    setError(null);
    try { await deleteSecret(key, scope); setConfirm(null); load(); if (keys.length <= 1) onEmpty(); }
    catch (e) { setError(errMsg(e)); }
  };

  return (
    <div className="flex flex-col gap-1">
      {keys.map((key) => (
        <div key={key} className="flex items-center gap-2 rounded-sm bg-accent px-2 py-1">
          <span className="flex-1 font-mono text-[10px] text-foreground">{key}</span>
          {confirm === key ? (
            <div className="flex gap-1">
              <Button size="xs" variant="ghost" className="text-red-400 hover:text-red-300" onClick={() => handleDelete(key)}>confirm</Button>
              <Button size="xs" variant="ghost" onClick={() => setConfirm(null)}>cancel</Button>
            </div>
          ) : (
            <Button size="xs" variant="ghost" className="text-muted-foreground hover:text-red-400" onClick={() => setConfirm(key)}>delete</Button>
          )}
        </div>
      ))}
      <div className="flex gap-1.5">
        <Input size="xs" className="w-2/5 font-mono" placeholder="KEY" value={newKey}
          onChange={(e) => setNewKey(e.target.value.replace(/[^A-Za-z0-9_]/g, "").toUpperCase())} />
        <Input size="xs" type="password" className="flex-1 font-mono" placeholder="value" value={newValue}
          onChange={(e) => setNewValue(e.target.value)} onKeyDown={(e) => e.key === "Enter" && handleAdd()} />
        <Button size="xs" onClick={handleAdd} disabled={!newKey.trim() || !newValue.trim()}>Add</Button>
      </div>
      {error && <div className={errBox}>{error}</div>}
    </div>
  );
}

export default function SettingsPanel() {
  const [scopes, setScopes] = useState<string[]>(["_platform"]);
  const [active, setActive] = useState("_platform");
  const [newScope, setNewScope] = useState("");
  const [error, setError] = useState<string | null>(null);

  const refreshScopes = useCallback(() => {
    listSecretScopes()
      .then((s) => setScopes(s.includes("_platform") ? s : ["_platform", ...s]))
      .catch((e) => setError(errMsg(e)));
  }, []);
  useEffect(refreshScopes, [refreshScopes]);

  const addScope = () => {
    const s = newScope.trim();
    if (!s || scopes.includes(s)) return;
    setScopes([...scopes, s]); setActive(s); setNewScope("");
  };

  return (
    <div className="flex flex-col gap-3 p-3">
      <AIProviderSection />
      <div className="my-1 border-t border-border" />
      <h3 className={heading}>Secrets</h3>
      <div className="flex flex-wrap items-center gap-1">
        {scopes.map((s) => (
          <Button key={s} size="xs" variant={active === s ? "default" : "outline"}
            onClick={() => setActive(s)}>{scopeLabel(s)}</Button>
        ))}
        <div className="flex gap-1">
          <Input size="xs" className="w-32 font-mono" placeholder="_mcp.name or app_id" value={newScope}
            onChange={(e) => setNewScope(e.target.value.replace(/[^a-z0-9._-]/gi, "").toLowerCase())}
            onKeyDown={(e) => e.key === "Enter" && addScope()} />
          <Button size="xs" variant="ghost" disabled={!newScope.trim()} onClick={addScope}>+</Button>
        </div>
      </div>
      <ScopeSection key={active} scope={active} onEmpty={refreshScopes} />
      {error && <div className={errBox}>{error}</div>}
    </div>
  );
}
