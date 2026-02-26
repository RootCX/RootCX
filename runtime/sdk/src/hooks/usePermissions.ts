import { useCallback, useEffect, useState } from "react";
import { type EffectivePermissions } from "../client";
import { useRuntimeClient } from "../components/RuntimeProvider";

export interface UsePermissionsResult {
  /** Resolved roles for the current user in this app. */
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

export function usePermissions(appId: string): UsePermissionsResult {
  const client = useRuntimeClient();
  const [data, setData] = useState<EffectivePermissions | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const fetchPerms = useCallback(() => {
    setLoading(true);
    setError(null);
    client
      .getPermissions(appId)
      .then((result) => {
        setData(result);
        setLoading(false);
      })
      .catch((err) => {
        setError(err instanceof Error ? err.message : String(err));
        setLoading(false);
      });
  }, [client, appId]);

  useEffect(() => {
    fetchPerms();
  }, [fetchPerms]);

  const can = useCallback(
    (permission: string): boolean => {
      if (!data) return false;
      return data.permissions.includes(permission) || data.permissions.includes("*");
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
