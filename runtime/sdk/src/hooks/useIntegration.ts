import { useCallback, useEffect, useState } from "react";
import { useRuntimeClient } from "../components/RuntimeProvider";

async function openExternal(url: string): Promise<boolean> {
  const t = (window as any).__TAURI_INTERNALS__;
  if (t?.invoke) {
    try {
      await t.invoke("plugin:shell|open", { path: url });
      return true;
    } catch (e) {
      console.warn("[useIntegration] Tauri shell open failed:", e);
    }
  } else {
    console.warn("[useIntegration] No __TAURI_INTERNALS__ — not in Tauri webview");
  }
  const w = window.open(url, "_blank");
  if (!w) console.warn("[useIntegration] window.open blocked");
  return w !== null;
}

export interface UseIntegrationResult {
  connected: boolean;
  loading: boolean;
  connect: () => Promise<{ type: string; schema?: Record<string, unknown> } | void>;
  submitCredentials: (credentials: Record<string, string>) => Promise<void>;
  disconnect: () => Promise<void>;
  call: (action: string, input?: Record<string, unknown>) => Promise<unknown>;
}

export function useIntegration(appId: string, integrationId: string): UseIntegrationResult {
  const client = useRuntimeClient();
  const [connected, setConnected] = useState(false);
  const [loading, setLoading] = useState(true);

  const checkStatus = useCallback(() =>
    client.integrationAuthStatus(appId, integrationId)
      .then(({ connected }) => setConnected(connected))
      .catch(() => setConnected(false)),
    [client, appId, integrationId],
  );

  useEffect(() => { checkStatus().finally(() => setLoading(false)); }, [checkStatus]);

  const connect = useCallback(async () => {
    const result = await client.integrationAuthStart(appId, integrationId);

    if (result.type === "redirect" && result.url) {
      const url = result.url as string;
      const opened = await openExternal(url);
      if (opened) {
        // Poll auth status — can't detect tab/browser close
        const poll = setInterval(async () => {
          const { connected: c } = await client.integrationAuthStatus(appId, integrationId).catch(() => ({ connected: false }));
          if (c) { clearInterval(poll); setConnected(true); }
        }, 2000);
        setTimeout(() => clearInterval(poll), 300_000);
      }
      return;
    }

    if (result.type === "credentials") {
      return { type: "credentials", schema: result.schema as Record<string, unknown> | undefined };
    }
  }, [client, appId, integrationId]);

  const submitCredentials = useCallback(async (credentials: Record<string, string>) => {
    await client.integrationAuthSubmit(appId, integrationId, credentials);
    await checkStatus();
  }, [client, appId, integrationId, checkStatus]);

  const disconnectFn = useCallback(async () => {
    await client.integrationAuthDisconnect(appId, integrationId);
    setConnected(false);
  }, [client, appId, integrationId]);

  const call = useCallback(
    (action: string, input?: Record<string, unknown>) =>
      client.callIntegration(appId, integrationId, action, input),
    [client, appId, integrationId],
  );

  return { connected, loading, connect, submitCredentials, disconnect: disconnectFn, call };
}
