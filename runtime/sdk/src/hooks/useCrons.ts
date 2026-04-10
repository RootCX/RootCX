import { useCallback, useEffect, useState } from "react";
import { useRuntimeClient } from "../components/RuntimeProvider";
import type { CronSchedule, CreateCronInput, UpdateCronInput } from "../client";

export interface UseCronsResult {
  data: CronSchedule[];
  loading: boolean;
  error: string | null;
  refetch: () => void;
  create: (input: CreateCronInput) => Promise<CronSchedule>;
  update: (id: string, patch: UpdateCronInput) => Promise<CronSchedule>;
  remove: (id: string) => Promise<void>;
  trigger: (id: string) => Promise<{ msgId: number }>;
}

export function useCrons(appId: string): UseCronsResult {
  const client = useRuntimeClient();
  const [data, setData] = useState<CronSchedule[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const fetchData = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setData(await client.listCrons(appId));
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, [client, appId]);

  useEffect(() => { fetchData(); }, [fetchData]);

  const create = useCallback(
    async (input: CreateCronInput): Promise<CronSchedule> => {
      const created = await client.createCron(appId, input);
      setData((prev) => [created, ...prev]);
      return created;
    },
    [client, appId],
  );

  const update = useCallback(
    async (id: string, patch: UpdateCronInput): Promise<CronSchedule> => {
      const updated = await client.updateCron(appId, id, patch);
      setData((prev) => prev.map((c) => (c.id === id ? updated : c)));
      return updated;
    },
    [client, appId],
  );

  const remove = useCallback(
    async (id: string): Promise<void> => {
      await client.deleteCron(appId, id);
      setData((prev) => prev.filter((c) => c.id !== id));
    },
    [client, appId],
  );

  const trigger = useCallback(
    (id: string) => client.triggerCron(appId, id),
    [client, appId],
  );

  return { data, loading, error, refetch: fetchData, create, update, remove, trigger };
}
