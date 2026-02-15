import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { OsStatus } from "../types";

const POLL_INTERVAL_MS = 3_000;

/**
 * Polls the Runtime for OS status via the Tauri `get_os_status` command.
 * Returns the latest status, a loading flag, and any error message.
 */
export function useOsStatus() {
  const [status, setStatus] = useState<OsStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const fetchStatus = useCallback(async () => {
    try {
      const result = await invoke<OsStatus>("get_os_status");
      setStatus(result);
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    fetchStatus();
    const id = setInterval(fetchStatus, POLL_INTERVAL_MS);
    return () => clearInterval(id);
  }, [fetchStatus]);

  return { status, loading, error };
}
