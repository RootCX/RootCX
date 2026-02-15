import { useCallback, useEffect, useRef, useState } from "react";
import { type OsStatus, RuntimeClient } from "../client";

export interface UseRuntimeStatusResult {
  /** Current runtime status, or null if not yet fetched. */
  status: OsStatus | null;
  /** True while polling. */
  loading: boolean;
  /** True if the daemon is reachable. */
  connected: boolean;
  /** Error message from the last poll. */
  error: string | null;
  /** Force an immediate re-fetch. */
  refetch: () => void;
}

/**
 * React hook that polls the Runtime daemon status at a regular interval.
 *
 * ```tsx
 * const { status, connected } = useRuntimeStatus({ pollInterval: 3000 });
 * ```
 */
export function useRuntimeStatus(opts?: {
  pollInterval?: number;
  baseUrl?: string;
}): UseRuntimeStatusResult {
  const interval = opts?.pollInterval ?? 3000;
  const clientRef = useRef(new RuntimeClient({ baseUrl: opts?.baseUrl }));
  const [status, setStatus] = useState<OsStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const [connected, setConnected] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const fetchStatus = useCallback(async () => {
    try {
      const s = await clientRef.current.status();
      setStatus(s);
      setConnected(true);
      setError(null);
    } catch (e) {
      setConnected(false);
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    fetchStatus();
    const id = setInterval(fetchStatus, interval);
    return () => clearInterval(id);
  }, [fetchStatus, interval]);

  return { status, loading, connected, error, refetch: fetchStatus };
}
