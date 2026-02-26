import { useState, useEffect, useSyncExternalStore } from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import { Button } from "@/components/ui/button";
import { Check } from "lucide-react";
import { cn } from "@/lib/utils";
import {
  AI_PROVIDERS,
  AWS_AUTH_MODES,
  envKeysForProvider,
  defaultAiConfig,
  aiConfigStore,
  type AIProvider,
  type AwsAuthMode,
} from "@/lib/ai-models";
import { setSecret, saveAiConfig } from "@/core/api";
import { workspace } from "@/core/studio";

type Step = "provider" | "key" | "saving";

let resolve: ((ok: boolean) => void) | null = null;
let setDialogState: ((v: { open: boolean; preselect?: string }) => void) | null = null;

export function showAISetupDialog(preselectedProvider?: string): Promise<boolean> {
  return new Promise((res) => {
    resolve = res;
    setDialogState?.({ open: true, preselect: preselectedProvider });
  });
}

export function AISetupDialogPortal() {
  const [state, setState] = useState({ open: false, preselect: undefined as string | undefined });
  useEffect(() => { setDialogState = setState; return () => { setDialogState = null; }; }, []);
  if (!state.open) return null;
  return createPortal(
    <AISetupWizard
      preselect={state.preselect}
      onDone={(ok) => { setState({ open: false }); resolve?.(ok); resolve = null; }}
    />,
    document.body,
  );
}

function AISetupWizard({ preselect, onDone }: { preselect?: string; onDone: (ok: boolean) => void }) {
  const currentConfig = useSyncExternalStore(aiConfigStore.subscribe, aiConfigStore.getSnapshot);
  const [step, setStep] = useState<Step>("provider");
  const [selected, setSelected] = useState<AIProvider | null>(null);
  const [secrets, setSecrets] = useState<Record<string, string>>({});
  const [error, setError] = useState<string | null>(null);
  const [awsAuthMode, setAwsAuthMode] = useState<AwsAuthMode>("apikey");
  const [savingStatus, setSavingStatus] = useState("");

  useEffect(() => {
    if (!preselect) return;
    const match = AI_PROVIDERS.find((provider) => provider.id === preselect);
    if (match) { setSelected(match); setStep("key"); }
  }, [preselect]);

  const selectProvider = (p: AIProvider) => {
    setSelected(p);
    setSecrets({});
    setError(null);
    setStep("key");
  };

  const activeEnvKeys = selected ? envKeysForProvider(selected, selected.id === "bedrock" ? awsAuthMode : undefined) : [];
  const allFilled = activeEnvKeys.length > 0 && activeEnvKeys.every((k) => secrets[k]?.trim());

  const save = async () => {
    if (!selected) return;
    setError(null);
    setStep("saving");

    try {
      setSavingStatus("Encrypting credentials...");
      for (const key of activeEnvKeys) {
        await setSecret(key, secrets[key].trim());
      }
      setSecrets({});

      setSavingStatus("Saving configuration...");
      await saveAiConfig(defaultAiConfig(selected.id));

      setSavingStatus("Starting AI engine...");
      await invoke("forge_reload_config").catch(() => {});
      await aiConfigStore.refresh();
      onDone(true);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setStep("key");
    }
  };

  const canDismiss = step !== "saving";

  return (
    <div className="fixed inset-0 z-50 flex items-start justify-center pt-[20vh]" onClick={() => canDismiss && onDone(false)}>
      <div className="absolute inset-0 bg-black/50" />
      <div
        className="relative max-h-[60vh] w-full max-w-md overflow-y-auto rounded-lg border border-border bg-card shadow-2xl"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="px-4 py-3">
          {step === "provider" && (
            <>
              <div className="mb-3 text-xs text-muted-foreground">Configure AI Provider</div>
              <div className="flex flex-col gap-1.5">
                {AI_PROVIDERS.map((p) => (
                  <button
                    key={p.id}
                    className={cn(
                      "flex items-center gap-3 rounded-md border px-3 py-2 text-left transition-colors",
                      "border-border bg-background hover:border-primary/50 hover:bg-accent",
                    )}
                    onClick={() => selectProvider(p)}
                  >
                    <span className="flex-1 text-sm font-medium text-foreground">{p.name}</span>
                    {currentConfig?.provider === p.id && (
                      <span className="flex items-center gap-1 text-[10px] text-green-400">
                        <Check className="h-3 w-3" /> active
                      </span>
                    )}
                  </button>
                ))}
              </div>
            </>
          )}

          {step === "key" && selected && (
            <>
              <div className="mb-3 flex items-center text-xs text-muted-foreground">
                <button className="hover:text-foreground" onClick={() => setStep("provider")}>
                  &larr; {selected.name}
                </button>
              </div>

              {selected.id === "bedrock" && (
                <div className="mb-3 flex gap-2">
                  {(Object.entries(AWS_AUTH_MODES) as [AwsAuthMode, typeof AWS_AUTH_MODES[AwsAuthMode]][]).map(([mode, { label }]) => (
                    <button
                      key={mode}
                      className={cn(
                        "rounded-md border px-2.5 py-1 text-[10px] font-medium transition-colors",
                        awsAuthMode === mode
                          ? "border-primary bg-primary/10 text-primary"
                          : "border-border text-muted-foreground hover:text-foreground",
                      )}
                      onClick={() => { setAwsAuthMode(mode); setSecrets({}); }}
                    >
                      {label}
                    </button>
                  ))}
                </div>
              )}

              <div className="flex flex-col gap-2">
                {activeEnvKeys.map((envKey, i) => (
                  <label key={envKey} className="flex flex-col gap-1">
                    <span className="text-[10px] font-medium text-muted-foreground">{envKey}</span>
                    <input
                      type="password"
                      autoFocus={i === 0}
                      className="w-full rounded-md border border-input bg-background px-3 py-2 font-mono text-sm text-foreground placeholder:text-muted-foreground focus:border-ring focus:outline-none"
                      value={secrets[envKey] ?? ""}
                      onChange={(e) => setSecrets((s) => ({ ...s, [envKey]: e.target.value }))}
                      onKeyDown={(e) => e.key === "Enter" && allFilled && save()}
                    />
                  </label>
                ))}
              </div>
              <div className="mt-3">
                <Button size="sm" className="h-7 text-xs" onClick={() => save()} disabled={!allFilled}>
                  Save
                </Button>
              </div>
            </>
          )}

          {step === "saving" && (
            <div className="flex flex-col items-center gap-3 py-8">
              <div className="h-5 w-5 animate-spin rounded-full border-2 border-primary border-t-transparent" />
              <span className="text-sm text-foreground">{savingStatus}</span>
            </div>
          )}

          {error && (
            <div className="mt-2 rounded-md border border-red-800 bg-red-950 px-2 py-1 text-[10px] text-red-300">
              {error}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
