import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button } from "@/components/ui/button";

export default function SettingsPanel() {
  const [keys, setKeys] = useState<string[]>([]);
  const [newKey, setNewKey] = useState("");
  const [newValue, setNewValue] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null);

  const refresh = useCallback(() => {
    invoke<string[]>("list_platform_secrets")
      .then(setKeys)
      .catch((e) => setError(String(e)));
  }, []);

  useEffect(() => { refresh(); }, [refresh]);

  const handleAdd = async () => {
    if (!newKey.trim() || !newValue.trim()) return;
    setError(null);
    try {
      await invoke("set_platform_secret", { key: newKey.trim(), value: newValue.trim() });
      setNewKey("");
      setNewValue("");
      refresh();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const handleDelete = async (key: string) => {
    setError(null);
    try {
      await invoke("delete_platform_secret", { key });
      setConfirmDelete(null);
      refresh();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  const inputClass =
    "rounded-md border border-input bg-background px-2 py-1 font-mono text-[10px] text-foreground placeholder:text-muted-foreground focus:border-ring focus:outline-none";

  return (
    <div className="flex flex-col gap-3 p-3">
      <h3 className="text-xs font-semibold uppercase tracking-wider text-primary">
        Platform Secrets
      </h3>
      <p className="text-[10px] text-muted-foreground">
        Environment variables injected into all workers and the AI Forge.
      </p>

      {keys.length > 0 && (
        <div className="flex flex-col gap-1">
          {keys.map((key) => (
            <div key={key} className="flex items-center gap-2 rounded-sm bg-accent px-2 py-1">
              <span className="flex-1 font-mono text-[10px] text-foreground">{key}</span>
              {confirmDelete === key ? (
                <div className="flex gap-1">
                  <button
                    className="text-[10px] text-red-400 hover:text-red-300"
                    onClick={() => handleDelete(key)}
                  >
                    confirm
                  </button>
                  <button
                    className="text-[10px] text-muted-foreground hover:text-foreground"
                    onClick={() => setConfirmDelete(null)}
                  >
                    cancel
                  </button>
                </div>
              ) : (
                <button
                  className="text-[10px] text-muted-foreground hover:text-red-400"
                  onClick={() => setConfirmDelete(key)}
                >
                  delete
                </button>
              )}
            </div>
          ))}
        </div>
      )}

      {keys.length === 0 && (
        <span className="text-[10px] text-muted-foreground">No secrets configured.</span>
      )}

      <div className="my-1 border-t border-border" />

      <h3 className="text-xs font-semibold uppercase tracking-wider text-primary">
        Add Secret
      </h3>

      <label className="flex flex-col gap-1">
        <span className="text-[10px] font-medium text-muted-foreground">Key</span>
        <input
          type="text"
          className={inputClass}
          placeholder="ANTHROPIC_API_KEY"
          value={newKey}
          onChange={(e) => setNewKey(e.target.value.replace(/[^A-Za-z0-9_]/g, "").toUpperCase())}
        />
      </label>

      <label className="flex flex-col gap-1">
        <span className="text-[10px] font-medium text-muted-foreground">Value</span>
        <input
          type="password"
          className={inputClass}
          placeholder="sk-..."
          value={newValue}
          onChange={(e) => setNewValue(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && handleAdd()}
        />
      </label>

      <Button size="sm" className="h-7 text-[10px]" onClick={handleAdd} disabled={!newKey.trim() || !newValue.trim()}>
        Save Secret
      </Button>

      {error && (
        <div className="rounded-md border border-red-800 bg-red-950 px-2 py-1 text-[10px] text-red-300">
          {error}
        </div>
      )}
    </div>
  );
}
