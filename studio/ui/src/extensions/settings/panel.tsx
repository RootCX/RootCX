import { useState, useEffect, useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { aiConfigStore } from "@/lib/ai-models";
import { showAISetupDialog } from "@/components/ai-setup-dialog";
import { saveAiConfig } from "@/core/api";

export default function SettingsPanel() {
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
    <div className="flex flex-col gap-3 p-3">
      <h3 className="text-xs font-semibold uppercase tracking-wider text-primary">AI Provider</h3>
      {aiConfig ? (
        <>
          <div className="flex items-center gap-2">
            <span className="h-2 w-2 rounded-full bg-green-500" />
            <span className="flex-1 text-[10px] text-foreground">{aiConfigStore.providerName()}</span>
            <Button size="xs" variant="link" onClick={() => showAISetupDialog()}>Change</Button>
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
        <Button size="xs" onClick={() => showAISetupDialog()}>Configure AI Provider</Button>
      )}
    </div>
  );
}
