import { useEffect, useRef } from "react";

interface BottomPanelProps {
  logs: string[];
  isRunning: boolean;
  isOpen: boolean;
  onToggle: () => void;
  onClear: () => void;
  onStop: () => void;
}

export default function BottomPanel({
  logs,
  isRunning,
  isOpen,
  onToggle,
  onClear,
  onStop,
}: BottomPanelProps) {
  const logsEndRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    logsEndRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [logs]);

  return (
    <div className={`bottom-panel ${isOpen ? "bottom-panel-open" : "bottom-panel-collapsed"}`}>
      <div className="bottom-panel-header" onClick={onToggle}>
        <div className="bottom-panel-tabs">
          <span className="bottom-panel-tab bottom-panel-tab-active">
            Console
            {isRunning && <span className="bottom-panel-running-dot" />}
          </span>
        </div>
        <div className="bottom-panel-actions">
          {isRunning && (
            <button
              className="bottom-panel-btn bottom-panel-btn-stop"
              onClick={(e) => { e.stopPropagation(); onStop(); }}
            >
              Stop
            </button>
          )}
          <button
            className="bottom-panel-btn"
            onClick={(e) => { e.stopPropagation(); onClear(); }}
          >
            Clear
          </button>
          <button className="bottom-panel-btn bottom-panel-chevron" onClick={(e) => { e.stopPropagation(); onToggle(); }}>
            {isOpen ? "▼" : "▲"}
          </button>
        </div>
      </div>
      {isOpen && (
        <div className="bottom-panel-logs">
          {logs.length === 0 ? (
            <div className="bottom-panel-empty">No output yet</div>
          ) : (
            logs.map((line, i) => (
              <div
                key={i}
                className={`bottom-panel-line${line.startsWith("[stderr]") ? " bottom-panel-line-err" : ""}${line.startsWith("[error]") ? " bottom-panel-line-err" : ""}`}
              >
                {line}
              </div>
            ))
          )}
          <div ref={logsEndRef} />
        </div>
      )}
    </div>
  );
}
