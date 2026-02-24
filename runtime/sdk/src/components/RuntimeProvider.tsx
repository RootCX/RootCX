import { createContext, useContext, useRef, type ReactNode } from "react";
import { RuntimeClient } from "../client";

export const REFRESH_KEY = "rootcx_refresh_token";

const RuntimeCtx = createContext<RuntimeClient | null>(null);

export interface RuntimeProviderProps {
  baseUrl?: string;
  children: ReactNode;
}

export function RuntimeProvider({ baseUrl, children }: RuntimeProviderProps) {
  const clientRef = useRef<RuntimeClient | null>(null);
  if (!clientRef.current) {
    const client = new RuntimeClient({ baseUrl });
    localStorage.removeItem("rootcx_access_token");
    const refresh = localStorage.getItem(REFRESH_KEY);
    if (refresh) client.setTokens(null, refresh);
    clientRef.current = client;
  }
  return <RuntimeCtx.Provider value={clientRef.current}>{children}</RuntimeCtx.Provider>;
}

export function useRuntimeClient(): RuntimeClient {
  const ctx = useContext(RuntimeCtx);
  if (!ctx) throw new Error("useRuntimeClient must be used inside <RuntimeProvider>");
  return ctx;
}
