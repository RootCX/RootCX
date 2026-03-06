import { useState, useEffect, useSyncExternalStore } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { Dialog, DialogContent, DialogHeader, DialogBody, DialogFooter, DialogTitle, DialogDescription } from "@/components/ui/dialog";
import { useProjectContext } from "@/components/layout/app-context";
import {
  subscribe, getSnapshot, loadProject, deploy, undeploy, bind, updateConfig, unbind, refresh,
  type Integration,
} from "./store";

const heading = "text-xs font-semibold uppercase tracking-wider text-primary";
const errBox = "rounded-md border border-red-800 bg-red-950 px-2 py-1 text-[10px] text-red-300";

function ConfigForm({ schema, onSubmit, submitLabel }: {
  schema: Record<string, unknown>;
  onSubmit: (values: Record<string, string>) => void;
  submitLabel: string;
}) {
  const props = (schema as any).properties ?? {};
  const required = new Set<string>((schema as any).required ?? []);
  const [values, setValues] = useState<Record<string, string>>({});
  const [saving, setSaving] = useState(false);

  const allFilled = Object.keys(props).every(k => !required.has(k) || values[k]?.trim());

  const handleSubmit = async () => {
    setSaving(true);
    try { await onSubmit(values); } finally { setSaving(false); }
  };

  return (
    <div className="flex flex-col gap-1.5">
      {Object.entries(props).map(([key, def]: [string, any]) => (
        <label key={key} className="flex flex-col gap-0.5">
          <span className="text-[10px] font-medium text-muted-foreground">
            {key}{required.has(key) && <span className="text-red-400"> *</span>}
          </span>
          <Input
            size="xs"
            type={/secret|token|key/i.test(key) ? "password" : "text"}
            className="font-mono"
            placeholder={def.description ?? key}
            value={values[key] ?? ""}
            onChange={(e) => setValues({ ...values, [key]: e.target.value })}
          />
        </label>
      ))}
      <Button size="xs" onClick={handleSubmit} disabled={!allFilled || saving}>
        {saving ? "..." : submitLabel}
      </Button>
    </div>
  );
}

type Status = "available" | "deployed" | "bound";

function platformSecretKeys(schema: Record<string, unknown> | null): string[] {
  if (!schema) return [];
  const props = (schema as any).properties ?? {};
  return Object.values(props).map((def: any) => def.platformSecret).filter(Boolean);
}

function IntegrationCard({ integration, status, hasApp }: {
  integration: Integration;
  status: Status;
  hasApp: boolean;
}) {
  const active = status !== "available";
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [showConfig, setShowConfig] = useState(false);
  const [confirmRemove, setConfirmRemove] = useState(false);

  const secretKeys = platformSecretKeys(integration.configSchema);

  const doRemove = async () => {
    setConfirmRemove(false);
    setBusy(true);
    setError(null);
    try {
      if (status === "bound") await unbind(integration.id);
      await undeploy(integration.id);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const toggle = async () => {
    if (active) {
      if (secretKeys.length > 0) { setConfirmRemove(true); return; }
      await doRemove();
    } else {
      setBusy(true);
      setError(null);
      try {
        await deploy(integration.id);
        if (!integration.configSchema && hasApp) await bind(integration.id);
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
      } finally {
        setBusy(false);
      }
    }
  };

  return (
    <div className="rounded-md border border-border bg-accent/30 p-2">
      <div className="flex items-center gap-2">
        <div className="flex-1">
          <div className="flex items-center gap-1.5">
            <span className="text-xs font-medium text-foreground">{integration.name}</span>
            <span className="text-[10px] text-muted-foreground">v{integration.version}</span>
          </div>
          {integration.description && (
            <p className="mt-0.5 text-[10px] text-muted-foreground">{integration.description}</p>
          )}
        </div>
        <div className="flex items-center gap-2">
          {active && hasApp && integration.configSchema && (
            <button
              className="text-[10px] text-muted-foreground hover:text-foreground"
              onClick={() => setShowConfig(!showConfig)}
            >
              {showConfig ? "Hide" : "Config"}
            </button>
          )}
          <Switch checked={active} disabled={busy} onCheckedChange={toggle} />
        </div>
      </div>

      {status === "deployed" && hasApp && integration.configSchema && (
        <div className="mt-2 border-t border-border pt-2">
          <ConfigForm
            schema={integration.configSchema}
            onSubmit={(config) => bind(integration.id, config)}
            submitLabel="Activate"
          />
        </div>
      )}

      {showConfig && status === "bound" && integration.configSchema && (
        <div className="mt-2 border-t border-border pt-2">
          <ConfigForm
            schema={integration.configSchema}
            onSubmit={(config) => updateConfig(integration.id, config)}
            submitLabel="Update"
          />
        </div>
      )}

      <Dialog open={confirmRemove} onOpenChange={setConfirmRemove}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Remove {integration.name}?</DialogTitle>
            <DialogDescription>
              This integration will be uninstalled. The following platform secrets will also be deleted:
            </DialogDescription>
          </DialogHeader>
          <DialogBody>
            <ul className="list-inside list-disc font-mono text-xs text-muted-foreground">
              {secretKeys.map(k => <li key={k}>{k}</li>)}
            </ul>
          </DialogBody>
          <DialogFooter>
            <Button size="xs" variant="ghost" onClick={() => setConfirmRemove(false)}>Cancel</Button>
            <Button size="xs" variant="destructive" onClick={doRemove}>Remove</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {error && <div className={`mt-1.5 ${errBox}`}>{error}</div>}
    </div>
  );
}

export default function IntegrationsPanel() {
  const { projectPath } = useProjectContext();
  const { appId, catalog, installed, bindings, loading, error } = useSyncExternalStore(subscribe, getSnapshot);

  useEffect(() => { if (projectPath) loadProject(projectPath); }, [projectPath]);

  const boundIds = new Set(bindings.filter(b => b.enabled).map(b => b.integrationId));
  const statusOf = (id: string): Status =>
    boundIds.has(id) ? "bound" : installed.has(id) ? "deployed" : "available";

  return (
    <div className="flex flex-col gap-3 p-3">
      <h3 className={heading}>Integrations</h3>
      {loading && <span className="text-[10px] text-muted-foreground">Loading...</span>}
      {error && <div className={errBox}>{error}</div>}
      {!loading && catalog.length === 0 && (
        <span className="text-[10px] text-muted-foreground">No integrations available.</span>
      )}
      {catalog.map(i => (
        <IntegrationCard key={i.id} integration={i} status={statusOf(i.id)} hasApp={!!appId} />
      ))}
    </div>
  );
}
