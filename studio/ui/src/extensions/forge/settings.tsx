import { useState, useEffect, useSyncExternalStore } from "react";
import { subscribe, getSnapshot, loadProviders, loadConfig } from "./store";
import { Button } from "@/components/ui/button";
import { useProjectContext } from "@/components/layout/app-context";
import { invoke } from "@tauri-apps/api/core";
import type { Config } from "@opencode-ai/sdk";

function InstructionsSection() {
  const [files, setFiles] = useState<string[] | null>(null);

  useEffect(() => {
    invoke<string[]>("resolve_instructions").then(setFiles).catch(() => setFiles([]));
  }, []);

  return (
    <div className="flex flex-col gap-1.5">
      <h3 className="text-xs font-semibold uppercase tracking-wider text-primary">Instructions</h3>
      {files === null ? (
        <span className="text-[10px] text-muted-foreground">Resolving...</span>
      ) : files.length === 0 ? (
        <span className="text-[10px] text-yellow-400">No instruction files found.</span>
      ) : (
        <div className="flex flex-col gap-0.5">
          {files.map((f) => (
            <span key={f} className="rounded-sm bg-accent px-1.5 py-0.5 font-mono text-[10px] text-foreground">{f}</span>
          ))}
        </div>
      )}
    </div>
  );
}

export default function ForgeSettings() {
  const { providers, connectedProviders, currentConfig } =
    useSyncExternalStore(subscribe, getSnapshot);
  const { projectPath } = useProjectContext();

  const [providerId, setProviderId] = useState("");
  const [modelKey, setModelKey] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    loadProviders();
    loadConfig();
  }, []);

  useEffect(() => {
    if (!currentConfig?.model) return;
    const [p, m] = currentConfig.model.split("/");
    if (p && m) {
      setProviderId(p);
      setModelKey(m);
    }
  }, [currentConfig]);

  const selectedProvider = providers.find((p) => p.id === providerId);
  const models = selectedProvider ? Object.entries(selectedProvider.models) : [];

  const handleSave = async () => {
    setSaved(false);
    setError(null);

    if (!providerId || !modelKey) {
      setError("Select a provider and model.");
      return;
    }

    const config: Config = { model: `${providerId}/${modelKey}` };
    if (apiKey.trim()) {
      config.provider = { [providerId]: { options: { apiKey: apiKey.trim() } } };
    }

    try {
      await invoke("save_forge_config", {
        contents: JSON.stringify(config, null, 2),
        projectPath: projectPath || undefined,
      });
      setSaved(true);
      setTimeout(() => setSaved(false), 3000);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  const selectClass =
    "rounded-md border border-input bg-background px-2 py-1 font-mono text-[10px] text-foreground focus:border-ring focus:outline-none";

  return (
    <div className="flex flex-col gap-3 p-3">
      <h3 className="text-xs font-semibold uppercase tracking-wider text-primary">
        Configuration
      </h3>

      <label className="flex flex-col gap-1">
        <span className="text-[10px] font-medium text-muted-foreground">Provider</span>
        <select
          className={selectClass}
          value={providerId}
          onChange={(e) => { setProviderId(e.target.value); setModelKey(""); setSaved(false); }}
        >
          <option value="">Select provider...</option>
          {providers.map((p) => (
            <option key={p.id} value={p.id}>
              {p.name}{connectedProviders.includes(p.id) ? " (connected)" : ""}
            </option>
          ))}
        </select>
      </label>

      <label className="flex flex-col gap-1">
        <span className="text-[10px] font-medium text-muted-foreground">Model</span>
        <select
          className={selectClass}
          value={modelKey}
          onChange={(e) => { setModelKey(e.target.value); setSaved(false); }}
          disabled={!providerId}
        >
          <option value="">{providerId ? "Select model..." : "Pick a provider first"}</option>
          {models.map(([key, m]) => (
            <option key={key} value={key}>{m.name}</option>
          ))}
        </select>
      </label>

      <label className="flex flex-col gap-1">
        <span className="text-[10px] font-medium text-muted-foreground">API Key</span>
        <input
          type="password"
          className="rounded-md border border-input bg-background px-2 py-1 font-mono text-[10px] text-foreground placeholder:text-muted-foreground focus:border-ring focus:outline-none"
          placeholder="sk-..."
          value={apiKey}
          onChange={(e) => { setApiKey(e.target.value); setSaved(false); }}
        />
        {selectedProvider && selectedProvider.env.length > 0 && (
          <span className="text-[10px] text-muted-foreground">
            Or set env: {selectedProvider.env.join(", ")}
          </span>
        )}
      </label>

      <div className="flex items-center gap-2">
        <Button size="sm" className="h-7 text-[10px]" onClick={handleSave}>Save</Button>
        {saved && <span className="text-[10px] text-green-400">Saved</span>}
      </div>

      {error && (
        <div className="rounded-md border border-red-800 bg-red-950 px-2 py-1 text-[10px] text-red-300">
          {error}
        </div>
      )}

      <div className="my-1 border-t border-border" />

      <InstructionsSection />
    </div>
  );
}
