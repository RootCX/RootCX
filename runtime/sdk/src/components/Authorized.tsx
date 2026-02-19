import type { ReactNode } from "react";
import { usePermissionsContext } from "./PermissionsProvider";

export interface AuthorizedProps {
  action: string;
  entity: string;
  /** Rendered when the user lacks permission. Defaults to null (render nothing). */
  fallback?: ReactNode;
  children: ReactNode;
}

export function Authorized({ action, entity, fallback = null, children }: AuthorizedProps) {
  const { can, loading } = usePermissionsContext();
  if (loading) return null;
  return can(action, entity) ? <>{children}</> : <>{fallback}</>;
}
