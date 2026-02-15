import { useCallback, useEffect, useRef, useState } from "react";
import { RuntimeClient } from "../client";

export interface UseAppCollectionResult<T> {
  /** Current records (empty array while loading). */
  data: T[];
  /** True during the initial fetch or a refetch. */
  loading: boolean;
  /** Error message if the last operation failed. */
  error: string | null;
  /** Re-fetch the collection from the Runtime. */
  refetch: () => void;
  /** Create a new record and append it to `data`. */
  create: (record: Record<string, unknown>) => Promise<T>;
  /** Update an existing record by id. */
  update: (id: string, patch: Record<string, unknown>) => Promise<T>;
  /** Delete a record by id. */
  remove: (id: string) => Promise<void>;
}

/**
 * React hook that provides CRUD access to an app collection via the Runtime daemon.
 *
 * ```tsx
 * const { data, loading, create } = useAppCollection<Customer>("crm", "customers");
 * ```
 */
export function useAppCollection<T extends { id?: string } = Record<string, unknown>>(
  appId: string,
  entity: string,
  baseUrl?: string,
): UseAppCollectionResult<T> {
  const clientRef = useRef(new RuntimeClient({ baseUrl }));
  const [data, setData] = useState<T[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const fetchData = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const records = await clientRef.current.listRecords<T>(appId, entity);
      setData(records);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, [appId, entity]);

  useEffect(() => {
    fetchData();
  }, [fetchData]);

  const create = useCallback(
    async (record: Record<string, unknown>): Promise<T> => {
      const created = await clientRef.current.createRecord<T>(appId, entity, record);
      setData((prev) => [created, ...prev]);
      return created;
    },
    [appId, entity],
  );

  const update = useCallback(
    async (id: string, patch: Record<string, unknown>): Promise<T> => {
      const updated = await clientRef.current.updateRecord<T>(appId, entity, id, patch);
      setData((prev) => prev.map((r) => (r.id === id ? updated : r)));
      return updated;
    },
    [appId, entity],
  );

  const remove = useCallback(
    async (id: string): Promise<void> => {
      await clientRef.current.deleteRecord(appId, entity, id);
      setData((prev) => prev.filter((r) => r.id !== id));
    },
    [appId, entity],
  );

  return { data, loading, error, refetch: fetchData, create, update, remove };
}
