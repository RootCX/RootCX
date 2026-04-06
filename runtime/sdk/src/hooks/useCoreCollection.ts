import { useCallback, useEffect, useState } from "react";
import { useRuntimeClient } from "../components/RuntimeProvider";

export interface UseCoreCollectionResult<T> {
  data: T[];
  loading: boolean;
  error: string | null;
  refetch: () => void;
}

export function useCoreCollection<T = Record<string, unknown>>(
  entity: string,
): UseCoreCollectionResult<T> {
  const client = useRuntimeClient();
  const [data, setData] = useState<T[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const fetchData = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const records = await client.core().collection<T>(entity).list();
      setData(records);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, [client, entity]);

  useEffect(() => { fetchData(); }, [fetchData]);

  return { data, loading, error, refetch: fetchData };
}
