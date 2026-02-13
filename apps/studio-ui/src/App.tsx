import { useOsStatus } from "./hooks/useOsStatus";
import type { ServiceState } from "./types";
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
    </div>
  );
}
