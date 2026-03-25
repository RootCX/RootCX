import { useState, useEffect, useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import { RefreshCw, Plus, Trash2, MoreVertical, Check } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { ListRow } from "@/components/ui/list-row";
import { Dialog, DialogContent, DialogHeader, DialogFooter, DialogTitle, DialogDescription } from "@/components/ui/dialog";
import { llmStore, AI_PROVIDERS, AWS_AUTH_MODES, envKeysForProvider, defaultModelForProvider, type AwsAuthMode } from "@/lib/ai-models";
import { createLlmModel, deleteLlmModel, setDefaultLlmModel, setSecret } from "@/core/api";
import { cn } from "@/lib/utils";

const errMsg = (e: unknown) => (e instanceof Error ? e.message : String(e));

function AddForm({ onDone }: { onDone: () => void }) {
  const models = useSyncExternalStore(llmStore.subscribe, llmStore.getSnapshot);
  const [step, setStep] = useState<"provider" | "key">("provider");
  const [providerId, setProviderId] = useState("");
  const [secrets, setSecrets] = useState<Record<string, string>>({});
  const [awsAuthMode, setAwsAuthMode] = useState<AwsAuthMode>("apikey");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const provider = AI_PROVIDERS.find((p) => p.id === providerId);
  const activeEnvKeys = provider ? envKeysForProvider(provider, provider.id === "bedrock" ? awsAuthMode : undefined) : [];
  const allFilled = activeEnvKeys.length > 0 && activeEnvKeys.every((k) => secrets[k]?.trim());

  const save = async () => {
    if (!provider || !allFilled) return;
    setSaving(true); setError(null);
    try {
      for (const key of activeEnvKeys) await setSecret(key, secrets[key].trim());
      const isFirst = models.length === 0;
      await createLlmModel({
        id: provider.id,
        name: provider.name,
        provider: provider.id,
        model: defaultModelForProvider(provider.id),
        is_default: isFirst,
      });
      await invoke("forge_reload_config").catch(() => {});
      onDone();
    } catch (e) { setError(errMsg(e)); }
    finally { setSaving(false); }
  };

  if (step === "provider") {
    return (
      <div className="flex flex-col gap-1.5 rounded-md border border-border p-3">
        <span className="text-[10px] font-medium text-muted-foreground mb-1">Select provider</span>
        {AI_PROVIDERS.map((p) => (
          <button key={p.id} onClick={() => { setProviderId(p.id); setSecrets({}); setStep("key"); }}
            className="flex items-center gap-2 rounded-md px-2.5 py-1.5 text-left text-xs text-foreground hover:bg-white/[0.04] transition-colors">
            {p.name}
          </button>
        ))}
        <button className="text-[10px] text-muted-foreground hover:text-foreground mt-1" onClick={onDone}>Cancel</button>
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-2 rounded-md border border-border p-3">
      <div className="flex items-center text-[10px] text-muted-foreground">
        <button className="hover:text-foreground transition-colors" onClick={() => setStep("provider")}>&larr; {provider?.name}</button>
      </div>

      {provider?.id === "bedrock" && (
        <div className="flex gap-1.5">
          {(Object.entries(AWS_AUTH_MODES) as [AwsAuthMode, typeof AWS_AUTH_MODES[AwsAuthMode]][]).map(([mode, { label }]) => (
            <button key={mode} onClick={() => { setAwsAuthMode(mode); setSecrets({}); }}
              className={cn("rounded-md border px-2 py-0.5 text-[9px] font-medium transition-colors",
                awsAuthMode === mode ? "border-primary/50 bg-primary/10 text-primary" : "border-border text-muted-foreground hover:text-foreground")}>
              {label}
            </button>
          ))}
        </div>
      )}

      {activeEnvKeys.map((envKey, i) => (
        <Input key={envKey} size="xs" className="font-mono" type="password" placeholder={envKey}
          autoFocus={i === 0} value={secrets[envKey] ?? ""}
          onChange={(e) => setSecrets((s) => ({ ...s, [envKey]: e.target.value }))}
          onKeyDown={(e) => e.key === "Enter" && allFilled && save()} />
      ))}

      <div className="flex items-center gap-2">
        <Button size="xs" onClick={save} disabled={!allFilled || saving}>{saving ? "..." : "Add"}</Button>
        <button className="text-[10px] text-muted-foreground hover:text-foreground" onClick={onDone}>Cancel</button>
      </div>
      {error && <p className="text-[10px] text-red-400">{error}</p>}
    </div>
  );
}

function RowMenu({ onSetDefault, onDelete, isDefault }: { onSetDefault: () => void; onDelete: () => void; isDefault: boolean }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="relative">
      <Button size="icon-xs" variant="ghost" className="text-muted-foreground" onClick={() => setOpen(!open)}>
        <MoreVertical className="h-3 w-3" />
      </Button>
      {open && (
        <>
          <div className="fixed inset-0 z-10" onClick={() => setOpen(false)} />
          <div className="absolute right-0 top-full mt-1 z-20 min-w-[130px] rounded-md border border-border bg-popover py-1 shadow-md" onClick={() => setOpen(false)}>
            {!isDefault && (
              <button onClick={onSetDefault}
                className="flex w-full items-center gap-2 px-3 py-1.5 text-[11px] text-foreground hover:bg-accent transition-colors">
                <Check className="h-3 w-3" /> Set as default
              </button>
            )}
            <button onClick={onDelete}
              className="flex w-full items-center gap-2 px-3 py-1.5 text-[11px] text-red-400 hover:bg-accent transition-colors">
              <Trash2 className="h-3 w-3" /> Remove
            </button>
          </div>
        </>
      )}
    </div>
  );
}

export default function LlmModelsPanel() {
  const models = useSyncExternalStore(llmStore.subscribe, llmStore.getSnapshot);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [showAdd, setShowAdd] = useState(false);
  const [deleting, setDeleting] = useState<string | null>(null);

  const load = () => {
    setLoading(true);
    llmStore.refresh().catch((e) => setError(errMsg(e))).finally(() => setLoading(false));
  };
  useEffect(load, []);

  const handleDelete = async (id: string) => {
    setDeleting(null); setError(null);
    try { await deleteLlmModel(id); load(); }
    catch (e) { setError(errMsg(e)); }
  };

  const handleSetDefault = async (id: string) => {
    setError(null);
    try {
      await setDefaultLlmModel(id);
      await invoke("forge_reload_config").catch(() => {});
      load();
    } catch (e) { setError(errMsg(e)); }
  };

  return (
    <div className="flex h-full flex-col">
      <div className="flex shrink-0 items-center justify-between border-b border-border px-3 py-1.5">
        <span className="text-xs font-medium uppercase tracking-wider text-muted-foreground">LLM Models</span>
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

        {models.map((m) => (
          <ListRow key={m.id} className="px-3 py-1.5">
            <div className="flex-1 min-w-0">
              <div className="flex items-center gap-1.5">
                <span className="truncate text-xs text-foreground">{m.name}</span>
                {m.is_default && (
                  <span className="text-[9px] font-medium px-1.5 py-0.5 rounded bg-primary/10 text-primary">Default</span>
                )}
              </div>
              <span className="text-[9px] text-muted-foreground font-mono">{m.provider}:{m.model}</span>
            </div>
            <RowMenu isDefault={m.is_default} onSetDefault={() => handleSetDefault(m.id)} onDelete={() => setDeleting(m.id)} />
          </ListRow>
        ))}

        {loading && models.length === 0 && (
          <p className="animate-pulse py-6 text-center text-xs text-muted-foreground">Loading...</p>
        )}
        {!loading && models.length === 0 && !showAdd && (
          <p className="py-8 text-center text-xs text-muted-foreground">No LLM models configured</p>
        )}
        {error && <p className="text-[10px] text-red-400">{error}</p>}
      </div>

      <Dialog open={!!deleting} onOpenChange={(open) => !open && setDeleting(null)}>
        <DialogContent className="max-w-xs">
          <DialogHeader>
            <DialogTitle>Remove LLM model</DialogTitle>
            <DialogDescription>
              Remove <strong>{models.find((m) => m.id === deleting)?.name ?? deleting}</strong>? Agents using this model will fall back to the default.
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button size="sm" variant="ghost" onClick={() => setDeleting(null)}>Cancel</Button>
            <Button size="sm" variant="destructive" onClick={() => handleDelete(deleting!)}>Remove</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
