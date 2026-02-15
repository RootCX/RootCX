import { useState, useRef, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";

interface AppLogsResult {
  lines: string[];
  offset: number;
}

export function useAppRunner() {
  const [isRunning, setIsRunning] = useState(false);
  const [logs, setLogs] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  const offsetRef = useRef(0);
  const pollingRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const pathRef = useRef("");

  const stopPolling = useCallback(() => {
    if (pollingRef.current) {
      clearInterval(pollingRef.current);
      pollingRef.current = null;
    }
  }, []);

  const startPolling = useCallback((projectPath: string) => {
    if (pollingRef.current) return;
    pollingRef.current = setInterval(async () => {
      try {
        const result = await invoke<AppLogsResult>("app_logs", {
          projectPath,
          since: offsetRef.current,
        });
        if (result.lines.length > 0) {
          setLogs((prev) => [...prev, ...result.lines]);
          offsetRef.current = result.offset;
        }
      } catch {
        try {
          const running = await invoke<string[]>("list_running_apps");
          if (!running.includes(projectPath)) {
            setIsRunning(false);
            stopPolling();
          }
        } catch {
          // ignore
        }
      }
    }, 500);
  }, [stopPolling]);

  const runApp = useCallback(async (projectPath: string) => {
    setError(null);
    setLogs([]);
    offsetRef.current = 0;
    pathRef.current = projectPath;
    try {
      await invoke("run_app", { projectPath });
      setIsRunning(true);
      startPolling(projectPath);
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      setError(msg);
      setLogs((prev) => [...prev, `[error] ${msg}`]);
    }
  }, [startPolling]);

  const stopApp = useCallback(async () => {
    try {
      await invoke("stop_app", { projectPath: pathRef.current });
      setIsRunning(false);
      stopPolling();
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      setError(msg);
      setLogs((prev) => [...prev, `[error] ${msg}`]);
    }
  }, [stopPolling]);

  const clearLogs = useCallback(() => {
    setLogs([]);
  }, []);

  return { isRunning, logs, error, runApp, stopApp, clearLogs };
}
