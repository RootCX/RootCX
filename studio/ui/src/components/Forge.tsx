import { useState, useRef, useEffect } from "react";
import { useForge, type ForgePhase } from "../hooks/useForge";

function phaseLabel(phase: ForgePhase): string {
  switch (phase) {
    case "idle":
      return "Ready";
    case "analyzing":
      return "Analyzing...";
    case "planning":
      return "Planning...";
    case "executing":
      return "Building...";
    case "verifying":
      return "Verifying...";
    case "done":
      return "Done";
    case "error":
      return "Error";
    case "stopped":
      return "Stopped";
  }
}

function phaseColor(phase: ForgePhase): string {
  switch (phase) {
    case "idle":
      return "#6b7280";
    case "done":
      return "#22c55e";
    case "error":
      return "#ef4444";
    case "stopped":
      return "#f97316";
    default:
      return "#3b82f6";
  }
}

interface ForgeProps {
  projectId: string;
  onRun?: (projectPath: string) => void;
  onStop?: () => void;
  isAppRunning?: boolean;
}

export default function Forge({ projectId, onRun, onStop, isAppRunning }: ForgeProps) {
  const {
    messages,
    phase,
    thinking,
    toolCalls,
    files,
    errors,
    isStreaming,
    sendMessage,
    stopBuild,
  } = useForge(projectId);

  const [input, setInput] = useState("");
  const [projectPath, setProjectPath] = useState("");
  const messagesEndRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages, thinking]);

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    if (!input.trim() || !projectPath.trim() || isStreaming) return;
    sendMessage(input.trim(), projectPath.trim());
    setInput("");
  };

  return (
    <div className="forge">
      {/* Phase indicator */}
      <div className="forge-header">
        <div className="forge-phase">
          <span
            className="badge-dot"
            style={{
              background: phaseColor(phase),
              animation:
                phase !== "idle" && phase !== "done" && phase !== "error" && phase !== "stopped"
                  ? "pulse 1.5s infinite"
                  : "none",
            }}
          />
          <span className="forge-phase-label">{phaseLabel(phase)}</span>
        </div>
        <div style={{ display: "flex", gap: "8px" }}>
          {phase === "done" && projectPath && !isAppRunning && (
            <button
              className="btn btn-run"
              onClick={() => onRun?.(projectPath)}
            >
              Run
            </button>
          )}
          {phase === "done" && isAppRunning && (
            <button
              className="btn btn-run-stop"
              onClick={() => onStop?.()}
            >
              Stop App
            </button>
          )}
          {isStreaming && (
            <button className="btn forge-stop-btn" onClick={stopBuild}>
              Stop
            </button>
          )}
        </div>
      </div>

      {/* Project path input */}
      <div className="forge-config">
        <input
          type="text"
          className="forge-input"
          placeholder="Project path (e.g. /Users/you/projects/my-app)"
          value={projectPath}
          onChange={(e) => setProjectPath(e.target.value)}
        />
      </div>

      {/* Messages area */}
      <div className="forge-messages">
        {messages.map((msg) => (
          <div key={msg.id} className={`forge-msg forge-msg-${msg.role}`}>
            <span className="forge-msg-role">
              {msg.role === "user" ? "You" : msg.role === "assistant" ? "Forge" : ""}
            </span>
            <span className="forge-msg-content">{msg.content}</span>
          </div>
        ))}

        {/* Live thinking */}
        {thinking && (
          <div className="forge-msg forge-msg-thinking">
            <span className="forge-msg-role">Forge</span>
            <span className="forge-msg-content">{thinking}</span>
          </div>
        )}

        {/* Active tool calls */}
        {toolCalls.length > 0 && (
          <div className="forge-tools">
            {toolCalls.map((tc, i) => (
              <span key={i} className="entity-tag">
                {tc.name}
              </span>
            ))}
          </div>
        )}

        {/* Errors */}
        {errors.map((err, i) => (
          <div key={i} className="error-box">
            {err}
          </div>
        ))}

        <div ref={messagesEndRef} />
      </div>

      {/* File changes summary */}
      {files.length > 0 && (
        <div className="forge-files">
          <span className="forge-files-label">Changed files:</span>
          {files.map((f, i) => (
            <span key={i} className="entity-tag">
              {f.action === "delete" ? "−" : f.action === "create" ? "+" : "~"} {f.path}
            </span>
          ))}
        </div>
      )}

      {/* Input */}
      <form className="forge-input-form" onSubmit={handleSubmit}>
        <input
          type="text"
          className="forge-input forge-chat-input"
          placeholder={isStreaming ? "Building..." : "Describe what to build..."}
          value={input}
          onChange={(e) => setInput(e.target.value)}
          disabled={isStreaming || !projectPath}
        />
        <button
          type="submit"
          className="btn btn-primary"
          disabled={isStreaming || !input.trim() || !projectPath}
        >
          Build
        </button>
      </form>
    </div>
  );
}
