import { createContext, useContext, useRef, type ReactNode } from "react";
import { RuntimeClient } from "../client";

/** @deprecated Access tokens are now kept in memory only. */
export const TOKEN_KEY = "rootcx_access_token";
export const REFRESH_KEY = "rootcx_refresh_token";

const RuntimeCtx = createContext<RuntimeClient | null>(null);

export interface RuntimeProviderProps {
  baseUrl?: string;
  children: ReactNode;
}

/** Shared RuntimeClient provider — restores tokens from localStorage on init. */
export function RuntimeProvider({ baseUrl, children }: RuntimeProviderProps) {
  const clientRef = useRef<RuntimeClient | null>(null);
  if (!clientRef.current) {
    const client = new RuntimeClient({ baseUrl });
    // Clear legacy access token — now kept in memory only
    localStorage.removeItem(TOKEN_KEY);
    const refresh = localStorage.getItem(REFRESH_KEY);
    if (refresh) {
      client.setTokens(null, refresh);
    }
    clientRef.current = client;
  }
  return <RuntimeCtx.Provider value={clientRef.current}>{children}</RuntimeCtx.Provider>;
}

/** Access the shared RuntimeClient from the nearest RuntimeProvider. */
export function useRuntimeClient(): RuntimeClient {
  const ctx = useContext(RuntimeCtx);
  if (!ctx) throw new Error("useRuntimeClient must be used inside <RuntimeProvider>");
  return ctx;
}
