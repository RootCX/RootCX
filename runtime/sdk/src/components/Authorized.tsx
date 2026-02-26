import type { ReactNode } from "react";
import { usePermissionsContext } from "./PermissionsProvider";

export interface AuthorizedProps {
  permission: string;
  /** Rendered when the user lacks permission. Defaults to null (render nothing). */
  fallback?: ReactNode;
  children: ReactNode;
}

export function Authorized({ permission, fallback = null, children }: AuthorizedProps) {
  const { can, loading } = usePermissionsContext();
  if (loading) return null;
  return can(permission) ? <>{children}</> : <>{fallback}</>;
}
