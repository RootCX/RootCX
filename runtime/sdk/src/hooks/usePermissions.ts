import { useCallback, useEffect, useState } from "react";
import { type EffectivePermissions } from "../client";
import { useRuntimeClient } from "../components/RuntimeProvider";

export interface UsePermissionsResult {
  /** Resolved roles for the current user. */
  roles: string[];
  /** Flat list of permission strings the user has. */
  permissions: string[];
  /** Check if the current user has a given permission. */
  can: (permission: string) => boolean;
  /** True while the initial fetch is in flight. */
  loading: boolean;
  /** Error message if the fetch failed. */
  error: string | null;
  /** Re-fetch permissions from the runtime. */
  refetch: () => void;
}

export function usePermissions(): UsePermissionsResult {
  const client = useRuntimeClient();
  const [data, setData] = useState<EffectivePermissions | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const fetchPerms = useCallback(() => {
    setLoading(true);
    setError(null);
    client
      .getPermissions()
      .then((result) => {
        setData(result);
        setLoading(false);
      })
      .catch((err) => {
        setError(err instanceof Error ? err.message : String(err));
        setLoading(false);
      });
  }, [client]);

  useEffect(() => {
    fetchPerms();
  }, [fetchPerms]);

  const can = useCallback(
    (permission: string): boolean => {
      if (!data) return false;
      return data.permissions.some((p) =>
        p === "*" || p === permission ||
        (p.endsWith(":*") && permission.startsWith(p.slice(0, -1))),
      );
    },
    [data],
  );

  return {
    roles: data?.roles ?? [],
    permissions: data?.permissions ?? [],
    can,
    loading,
    error,
    refetch: fetchPerms,
  };
}
