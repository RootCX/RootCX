import { useCallback, useEffect, useState } from "react";
import { useRuntimeClient } from "../components/RuntimeProvider";
import type { IdentityRecord, QueryOptions } from "../client";

export interface UseIdentityResult<T> {
  data: IdentityRecord<T>[];
  total: number;
  loading: boolean;
  error: string | null;
  refetch: () => void;
}

export function useIdentity<T extends { id?: string } = Record<string, unknown>>(
  identityKind: string,
  query?: QueryOptions,
): UseIdentityResult<T> {
  const client = useRuntimeClient();
  const [data, setData] = useState<IdentityRecord<T>[]>([]);
  const [total, setTotal] = useState(0);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const queryKey = identityKind + JSON.stringify(query);

  const fetchData = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const result = await client.identityQuery<T>(identityKind, query);
      setData(result.data);
      setTotal(result.total);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, [client, queryKey]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => { fetchData(); }, [fetchData]);

  return { data, total, loading, error, refetch: fetchData };
}
