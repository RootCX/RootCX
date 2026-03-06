import { useState, useEffect, useSyncExternalStore } from "react";
import { RefreshCw } from "lucide-react";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { ListRow } from "@/components/ui/list-row";
import { Dialog, DialogContent, DialogHeader, DialogBody, DialogFooter, DialogTitle, DialogDescription } from "@/components/ui/dialog";
import { useProjectContext } from "@/components/layout/app-context";
import {
  subscribe, getSnapshot, loadProject, refresh, deploy, undeploy, bind, unbind, saveConfig,
  type Integration,
} from "./store";

function ConfigForm({ schema, onSubmit, onCancel, submitLabel }: {
  schema: Record<string, unknown>;
  onSubmit: (values: Record<string, string>) => void;
  onCancel: () => void;
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
    <div className="flex flex-col gap-1.5 px-3 py-2">
      {Object.entries(props).map(([key, def]: [string, any]) => (
        <label key={key} className="flex flex-col gap-0.5">
          <span className="text-[10px] font-medium text-muted-foreground">
            {def.description ?? key}{required.has(key) && <span className="text-red-400"> *</span>}
          </span>
          <Input
            size="xs"
            type={/secret|token|key/i.test(key) ? "password" : "text"}
            className="font-mono"
            placeholder={key}
            value={values[key] ?? ""}
            onChange={(e) => setValues({ ...values, [key]: e.target.value })}
          />
        </label>
      ))}
      <div className="flex items-center gap-2">
        <Button size="xs" onClick={handleSubmit} disabled={!allFilled || saving}>
          {saving ? "..." : submitLabel}
        </Button>
        <button className="text-[10px] text-muted-foreground hover:text-foreground" onClick={onCancel}>Cancel</button>
      </div>
    </div>
  );
}

function platformSecretKeys(schema: Record<string, unknown> | null): string[] {
  if (!schema) return [];
  const props = (schema as any).properties ?? {};
  return Object.values(props).map((def: any) => def.platformSecret).filter(Boolean);
}

function BrowseCard({ integration }: { integration: Integration }) {
  const [expanded, setExpanded] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const handleAdd = async (config?: Record<string, string>) => {
    setBusy(true);
    setError(null);
    try {
      await deploy(integration.id);
      if (config) await saveConfig(integration.id, config);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally { setBusy(false); }
  };

  return (
    <div>
      <ListRow className="px-3 py-1.5">
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-1.5">
            <span className="text-xs font-medium">{integration.name}</span>
            <span className="text-[10px] text-muted-foreground">v{integration.version}</span>
          </div>
          {integration.description && (
            <p className="text-[10px] text-muted-foreground">{integration.description}</p>
          )}
        </div>
        {!expanded && (
          <Button size="xs" disabled={busy} onClick={() => integration.configSchema ? setExpanded(true) : handleAdd()}>
            {busy ? "..." : "Add"}
          </Button>
        )}
      </ListRow>

      {expanded && integration.configSchema && (
        <ConfigForm
          schema={integration.configSchema}
          submitLabel="Add"
          onSubmit={(config) => handleAdd(config)}
          onCancel={() => setExpanded(false)}
        />
      )}

      {error && <div className="px-3 py-1 text-[10px] text-red-400">{error}</div>}
    </div>
  );
}

function InstalledCard({ integration, configured, bound, hasApp }: {
  integration: Integration;
  configured: boolean;
  bound: boolean;
  hasApp: boolean;
}) {
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
      if (bound) await unbind(integration.id);
      await undeploy(integration.id);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally { setBusy(false); }
  };

  const toggleBind = async () => {
    setBusy(true);
    setError(null);
    try {
      if (bound) await unbind(integration.id);
      else await bind(integration.id);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally { setBusy(false); }
  };

  return (
    <div>
      <ListRow className="px-3 py-1.5">
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-1.5">
            <span className="text-xs font-medium">{integration.name}</span>
            <span className="text-[10px] text-muted-foreground">v{integration.version}</span>
            {integration.configSchema && !showConfig && (
              <>
                <span className="text-[10px] text-border">·</span>
                {configured
                  ? <span className="text-[10px] text-green-400">Configured</span>
                  : <span className="text-[10px] text-yellow-400">Setup required</span>}
              </>
            )}
          </div>
          {integration.description && (
            <p className="text-[10px] text-muted-foreground">{integration.description}</p>
          )}
        </div>
        <div className="flex items-center gap-1.5">
          {integration.configSchema && !showConfig && (
            <button className="text-[10px] text-muted-foreground hover:text-foreground" onClick={() => setShowConfig(true)}>
              {configured ? "Edit" : "Configure"}
            </button>
          )}
          <button
            className="text-[10px] text-muted-foreground hover:text-red-400"
            onClick={() => secretKeys.length > 0 ? setConfirmRemove(true) : doRemove()}
            disabled={busy}
          >
            Remove
          </button>
        </div>
      </ListRow>

      {showConfig && integration.configSchema && (
        <ConfigForm
          schema={integration.configSchema}
          submitLabel="Save"
          onSubmit={async (config) => { await saveConfig(integration.id, config); setShowConfig(false); }}
          onCancel={() => setShowConfig(false)}
        />
      )}

      {configured && hasApp && (
        <div className="flex items-center justify-between px-3 py-1">
          <span className="text-[10px] text-muted-foreground">Enable for this app</span>
          <Switch size="sm" checked={bound} disabled={busy} onCheckedChange={toggleBind} />
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
            <Button size="sm" variant="ghost" onClick={() => setConfirmRemove(false)}>Cancel</Button>
            <Button size="sm" variant="destructive" onClick={doRemove}>Remove</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {error && <div className="px-3 py-1 text-[10px] text-red-400">{error}</div>}
    </div>
  );
}

export default function IntegrationsPanel() {
  const { projectPath } = useProjectContext();
  const { appId, catalog, installed, configured, bindings, loading, error } = useSyncExternalStore(subscribe, getSnapshot);
  const [tab, setTab] = useState<"installed" | "browse">("installed");

  useEffect(() => { if (projectPath) loadProject(projectPath); }, [projectPath]);

  const boundIds = new Set(bindings.filter(b => b.enabled).map(b => b.integrationId));
  const installedList = catalog.filter(i => installed.has(i.id));
  const browseList = catalog.filter(i => !installed.has(i.id));

  return (
    <div className="flex h-full flex-col">
      <div className="flex shrink-0 items-center justify-between border-b border-border px-3 py-1.5">
        <span className="text-xs font-medium uppercase tracking-wider text-muted-foreground">Integrations</span>
        <Button size="icon-xs" variant="ghost" onClick={() => refresh()}>
          <RefreshCw className={cn("h-3.5 w-3.5", loading && "animate-spin")} />
        </Button>
      </div>

      <div className="flex shrink-0 border-b border-border">
        <button
          onClick={() => setTab("installed")}
          className={cn(
            "flex-1 px-3 py-1.5 text-[10px] font-medium uppercase tracking-wider",
            tab === "installed" ? "border-b-2 border-primary text-foreground" : "text-muted-foreground hover:text-foreground",
          )}
        >
          Installed{installedList.length > 0 && ` (${installedList.length})`}
        </button>
        <button
          onClick={() => setTab("browse")}
          className={cn(
            "flex-1 px-3 py-1.5 text-[10px] font-medium uppercase tracking-wider",
            tab === "browse" ? "border-b-2 border-primary text-foreground" : "text-muted-foreground hover:text-foreground",
          )}
        >
          Browse{browseList.length > 0 && ` (${browseList.length})`}
        </button>
      </div>

      {error && <div className="px-3 py-2 text-xs text-red-400">{error}</div>}
      {loading && catalog.length === 0 && (
        <div className="flex animate-pulse items-center justify-center p-6 text-xs text-muted-foreground">Loading...</div>
      )}

      <div className="flex-1 overflow-auto">
        {tab === "installed" && (
          installedList.length === 0 && !loading
            ? <div className="flex items-center justify-center p-6 text-xs text-muted-foreground">No integrations installed yet.</div>
            : installedList.map(i => (
              <InstalledCard
                key={i.id}
                integration={i}
                configured={configured.has(i.id)}
                bound={boundIds.has(i.id)}
                hasApp={!!appId}
              />
            ))
        )}

        {tab === "browse" && (
          browseList.length === 0 && !loading
            ? <div className="flex items-center justify-center p-6 text-xs text-muted-foreground">All integrations are installed.</div>
            : browseList.map(i => (
              <BrowseCard key={i.id} integration={i} />
            ))
        )}
      </div>
    </div>
  );
}
