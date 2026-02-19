import { createContext, useContext, type ReactNode } from "react";
import { usePermissions, type UsePermissionsResult } from "../hooks/usePermissions";

const PermissionsCtx = createContext<UsePermissionsResult | null>(null);

export interface PermissionsProviderProps {
  appId: string;
  baseUrl?: string;
  children: ReactNode;
}

/** Single fetch point — all descendant `Authorized` / `usePermissionsContext` share this data. */
export function PermissionsProvider({ appId, baseUrl, children }: PermissionsProviderProps) {
  const perms = usePermissions(appId, { baseUrl });
  return <PermissionsCtx.Provider value={perms}>{children}</PermissionsCtx.Provider>;
}

/** Read permissions from the nearest `PermissionsProvider`. */
export function usePermissionsContext(): UsePermissionsResult {
  const ctx = useContext(PermissionsCtx);
  if (!ctx) throw new Error("usePermissionsContext must be used inside <PermissionsProvider>");
  return ctx;
}
