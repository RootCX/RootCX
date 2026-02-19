import { useCallback, useEffect, useState } from "react";
import { type EffectivePermissions } from "../client";
import { useRuntimeClient } from "../components/RuntimeProvider";

export interface UsePermissionsResult {
  /** Resolved roles for the current user in this app. */
  roles: string[];
  /** Per-entity effective permissions. */
  permissions: Record<string, { actions: string[]; ownership: boolean }>;
  /** Check if the current user can perform an action on an entity. */
  can: (action: string, entity: string) => boolean;
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
    (action: string, entity: string): boolean => {
      if (!data) return false;
      // Check exact entity match
      const ep = data.permissions[entity];
      if (ep && (ep.actions.includes(action) || ep.actions.includes("*"))) {
        return true;
      }
      // Check wildcard entity
      const wildcard = data.permissions["*"];
      if (
        wildcard &&
        (wildcard.actions.includes(action) || wildcard.actions.includes("*"))
      ) {
        return true;
      }
      return false;
    },
    [data],
  );

  return {
    roles: data?.roles ?? [],
    permissions: data?.permissions ?? {},
    can,
    loading,
    error,
    refetch: fetchPerms,
  };
}
