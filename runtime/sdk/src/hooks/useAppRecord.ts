import { useCallback, useEffect, useRef, useState } from "react";
import { RuntimeClient } from "../client";

export interface UseAppRecordResult<T> {
  /** The fetched record, or null while loading. */
  data: T | null;
  /** True during fetch. */
  loading: boolean;
  /** Error message if the fetch failed. */
  error: string | null;
  /** Re-fetch the record. */
  refetch: () => void;
  /** Update the record (PATCH). */
  update: (patch: Record<string, unknown>) => Promise<T>;
  /** Delete the record. */
  remove: () => Promise<void>;
}

/**
 * React hook that fetches a single record by id from the Runtime daemon.
 *
 * ```tsx
 * const { data, update, remove } = useAppRecord<Customer>("crm", "customers", customerId);
 * ```
 */
export function useAppRecord<T = Record<string, unknown>>(
  appId: string,
  entity: string,
  id: string | null,
  baseUrl?: string,
): UseAppRecordResult<T> {
  const clientRef = useRef(new RuntimeClient({ baseUrl }));
  const [data, setData] = useState<T | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const fetchData = useCallback(async () => {
    if (!id) {
      setData(null);
      setLoading(false);
      return;
    }
    setLoading(true);
    setError(null);
    try {
      const record = await clientRef.current.getRecord<T>(appId, entity, id);
      setData(record);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, [appId, entity, id]);

  useEffect(() => {
    fetchData();
  }, [fetchData]);

  const update = useCallback(
    async (patch: Record<string, unknown>): Promise<T> => {
      if (!id) throw new Error("cannot update: no record id");
      const updated = await clientRef.current.updateRecord<T>(appId, entity, id, patch);
      setData(updated);
      return updated;
    },
    [appId, entity, id],
  );

  const remove = useCallback(async (): Promise<void> => {
    if (!id) throw new Error("cannot delete: no record id");
    await clientRef.current.deleteRecord(appId, entity, id);
    setData(null);
  }, [appId, entity, id]);

  return { data, loading, error, refetch: fetchData, update, remove };
}
