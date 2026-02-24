import { useCallback, useEffect, useState } from "react";
import { useRuntimeClient } from "../components/RuntimeProvider";
import type { QueryOptions } from "../client";

export interface UseAppCollectionResult<T> {
  data: T[];
  total: number;
  loading: boolean;
  error: string | null;
  refetch: () => void;
  create: (record: Record<string, unknown>) => Promise<T>;
  update: (id: string, patch: Record<string, unknown>) => Promise<T>;
  remove: (id: string) => Promise<void>;
}

export function useAppCollection<T extends { id?: string } = Record<string, unknown>>(
  appId: string,
  entity: string,
  query?: QueryOptions,
): UseAppCollectionResult<T> {
  const client = useRuntimeClient();
  const [data, setData] = useState<T[]>([]);
  const [total, setTotal] = useState(0);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const queryKey = JSON.stringify(query);

  const fetchData = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      if (query) {
        const result = await client.queryRecords<T>(appId, entity, query);
        setData(result.data);
        setTotal(result.total);
      } else {
        const records = await client.listRecords<T>(appId, entity);
        setData(records);
        setTotal(records.length);
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, [client, appId, entity, queryKey]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => { fetchData(); }, [fetchData]);

  const create = useCallback(
    async (record: Record<string, unknown>): Promise<T> => {
      const created = await client.createRecord<T>(appId, entity, record);
      setData((prev) => [created, ...prev]);
      setTotal((prev) => prev + 1);
      return created;
    },
    [client, appId, entity],
  );

  const update = useCallback(
    async (id: string, patch: Record<string, unknown>): Promise<T> => {
      const updated = await client.updateRecord<T>(appId, entity, id, patch);
      setData((prev) => prev.map((r) => (r.id === id ? updated : r)));
      return updated;
    },
    [client, appId, entity],
  );

  const remove = useCallback(
    async (id: string): Promise<void> => {
      await client.deleteRecord(appId, entity, id);
      setData((prev) => prev.filter((r) => r.id !== id));
      setTotal((prev) => Math.max(0, prev - 1));
    },
    [client, appId, entity],
  );

  return { data, total, loading, error, refetch: fetchData, create, update, remove };
}
