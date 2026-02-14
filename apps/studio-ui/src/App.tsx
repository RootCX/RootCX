import { useState, useCallback, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useOsStatus } from "./hooks/useOsStatus";
import { helloManifest } from "./manifests/hello";
import Forge from "./components/Forge";
import type { ServiceState, InstalledApp } from "./types";
import "./App.css";

function stateColor(state: ServiceState): string {
  switch (state) {
    case "online":
      return "#22c55e";
    case "starting":
      return "#eab308";
    case "stopping":
      return "#f97316";
    case "error":
      return "#ef4444";
    default:
      return "#6b7280";
  }
}

function Badge({ label, state }: { label: string; state: ServiceState }) {
  return (
    <div className="badge">
      <span className="badge-dot" style={{ background: stateColor(state) }} />
      <span className="badge-label">{label}</span>
      <span className="badge-state">{state.toUpperCase()}</span>
    </div>
  );
}

export default function App() {
  const { status, loading, error } = useOsStatus();
  const [apps, setApps] = useState<InstalledApp[]>([]);
  const [installing, setInstalling] = useState(false);
  const [message, setMessage] = useState<{
    text: string;
    type: "success" | "error";
  } | null>(null);

  const isOnline = status?.kernel.state === "online";

  const refreshApps = useCallback(async () => {
    try {
      const result = await invoke<InstalledApp[]>("list_apps");
      setApps(result);
    } catch {
      // Kernel might not be ready yet
    }
  }, []);

  useEffect(() => {
    if (isOnline) refreshApps();
  }, [isOnline, refreshApps]);

  const handleInstallHello = async () => {
    setInstalling(true);
    setMessage(null);
    try {
      const result = await invoke<string>("install_app", {
        manifestJson: JSON.stringify(helloManifest),
      });
      setMessage({ text: result, type: "success" });
      await refreshApps();
    } catch (err) {
      setMessage({
        text: err instanceof Error ? err.message : String(err),
        type: "error",
      });
    } finally {
      setInstalling(false);
    }
  };

  return (
    <div className="container">
      <header className="header">
        <h1>RootCX Studio</h1>
        <p className="subtitle">Operating System Status</p>
      </header>

      {loading && <p className="loading">Connecting to Kernel...</p>}

      {error && (
        <div className="error-box">
          <strong>Error:</strong> {error}
        </div>
      )}

      {status && (
        <div className="status-grid">
          <div className="card">
            <h2>Kernel</h2>
            <Badge label="Status" state={status.kernel.state} />
            <div className="detail">
              <span>Version</span>
              <code>{status.kernel.version}</code>
            </div>
          </div>

          <div className="card">
            <h2>PostgreSQL</h2>
            <Badge label="Status" state={status.postgres.state} />
            <div className="detail">
              <span>Port</span>
              <code>{status.postgres.port ?? "—"}</code>
            </div>
            <div className="detail">
              <span>Data Dir</span>
              <code className="path">
                {status.postgres.data_dir ?? "—"}
              </code>
            </div>
          </div>
        </div>
      )}

      {isOnline && (
        <section className="apps-section">
          <div className="section-header">
            <h2>Apps</h2>
            <button
              className="btn btn-primary"
              onClick={handleInstallHello}
              disabled={installing}
            >
              {installing ? "Installing..." : "Install Hello App"}
            </button>
          </div>

          {message && (
            <div className={`msg msg-${message.type}`}>{message.text}</div>
          )}

          {apps.length > 0 ? (
            <div className="app-list">
              {apps.map((app) => (
                <div key={app.id} className="app-card">
                  <div className="app-header">
                    <span className="app-name">{app.name}</span>
                    <code className="app-version">v{app.version}</code>
                  </div>
                  <div className="app-entities">
                    {app.entities.map((e) => (
                      <span key={e} className="entity-tag">
                        {e}
                      </span>
                    ))}
                  </div>
                </div>
              ))}
            </div>
          ) : (
            <p className="no-apps">No apps installed</p>
          )}
        </section>
      )}

      {isOnline && status?.forge.state === "online" && (
        <section className="apps-section">
          <div className="section-header">
            <h2>AI Forge</h2>
            <Badge label="Status" state={status.forge.state} />
          </div>
          <Forge projectId="studio-default" />
        </section>
      )}
    </div>
  );
}
