import { useCallback, useEffect, useRef, useState } from "react";
import { useRuntimeClient } from "../components/RuntimeProvider";
import type { IntegrationConnection } from "../client";

async function openExternal(url: string): Promise<boolean> {
  const t = (window as any).__TAURI_INTERNALS__;
  if (t?.invoke) {
    try {
      await t.invoke("plugin:shell|open", { path: url });
      return true;
    } catch (e) {
      console.warn("[useIntegration] Tauri shell open failed:", e);
    }
  }
  const w = window.open(url, "_blank");
  return w !== null;
}

export interface UseIntegrationResult {
  connected: boolean;
  connections: IntegrationConnection[];
  loading: boolean;
  connect: () => Promise<{ type: string; schema?: Record<string, unknown> } | void>;
  submitCredentials: (credentials: Record<string, string>) => Promise<void>;
  remove: (connectionId: string) => Promise<void>;
  call: (action: string, input?: Record<string, unknown>) => Promise<unknown>;
}

export function useIntegration(integrationId: string): UseIntegrationResult {
  const client = useRuntimeClient();
  const [connections, setConnections] = useState<IntegrationConnection[]>([]);
  const [loading, setLoading] = useState(true);
  const pollRef = useRef<ReturnType<typeof setInterval> | null>(null);

  useEffect(() => () => { if (pollRef.current) clearInterval(pollRef.current); }, []);

  const fetchConnections = useCallback(async () => {
    try {
      const conns = await client.listConnections(integrationId);
      setConnections(conns);
    } catch {
      setConnections([]);
    }
  }, [client, integrationId]);

  useEffect(() => { fetchConnections().finally(() => setLoading(false)); }, [fetchConnections]);

  const connect = useCallback(async () => {
    const result = await client.integrationAuthStart(integrationId);

    if (result.type === "redirect" && result.url) {
      const url = result.url as string;
      const opened = await openExternal(url);
      if (opened) {
        const baseline = connections.length;
        pollRef.current = setInterval(async () => {
          const conns = await client.listConnections(integrationId).catch(() => []);
          if (conns.length > baseline) {
            if (pollRef.current) clearInterval(pollRef.current);
            pollRef.current = null;
            setConnections(conns);
          }
        }, 2000);
        setTimeout(() => { if (pollRef.current) { clearInterval(pollRef.current); pollRef.current = null; } }, 300_000);
      }
      return;
    }

    if (result.type === "credentials") {
      return { type: "credentials", schema: result.schema as Record<string, unknown> | undefined };
    }
  }, [client, integrationId, connections.length]);

  const submitCredentials = useCallback(async (credentials: Record<string, string>) => {
    await client.integrationAuthSubmit(integrationId, credentials);
    await fetchConnections();
  }, [client, integrationId, fetchConnections]);

  const remove = useCallback(async (connectionId: string) => {
    await client.deleteConnection(integrationId, connectionId);
    await fetchConnections();
  }, [client, integrationId, fetchConnections]);

  const call = useCallback(
    (action: string, input?: Record<string, unknown>) =>
      client.callIntegration(integrationId, action, input),
    [client, integrationId],
  );

  return {
    connected: connections.length > 0,
    connections,
    loading,
    connect,
    submitCredentials,
    remove,
    call,
  };
}
