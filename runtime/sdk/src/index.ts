export { RuntimeClient, RuntimeApiError } from "./client";
export type {
  RuntimeClientOptions,
  AuthUser,
  LoginResponse,
  RegisterInput,
  RoleDefinition,
  RoleAssignment,
  EntityPermission,
  EffectivePermissions,
} from "./client";

export { RuntimeProvider, useRuntimeClient } from "./components/RuntimeProvider";
export type { RuntimeProviderProps } from "./components/RuntimeProvider";

export { useAppCollection } from "./hooks/useAppCollection";
export type { UseAppCollectionResult } from "./hooks/useAppCollection";

export { useAppRecord } from "./hooks/useAppRecord";
export type { UseAppRecordResult } from "./hooks/useAppRecord";

export { useAuth } from "./hooks/useAuth";
export type { UseAuthResult } from "./hooks/useAuth";

export { usePermissions } from "./hooks/usePermissions";
export type { UsePermissionsResult } from "./hooks/usePermissions";

export { PermissionsProvider, usePermissionsContext } from "./components/PermissionsProvider";
export type { PermissionsProviderProps } from "./components/PermissionsProvider";

export { Authorized } from "./components/Authorized";
export type { AuthorizedProps } from "./components/Authorized";

export { AuthGate } from "./components/AuthGate";
export type { AuthGateProps, AuthFormSlotProps } from "./components/AuthGate";
