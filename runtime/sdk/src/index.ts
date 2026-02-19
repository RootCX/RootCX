// Client
export { RuntimeClient, RuntimeApiError } from "./client";
export type {
  RuntimeClientOptions,
  OsStatus,
  InstalledApp,
  AppManifest,
  AuthUser,
  LoginResponse,
  RegisterInput,
  RpcCaller,
  RoleDefinition,
  RoleAssignment,
  EntityPermission,
  EffectivePermissions,
} from "./client";

// Provider
export { RuntimeProvider, useRuntimeClient } from "./components/RuntimeProvider";
export type { RuntimeProviderProps } from "./components/RuntimeProvider";

// Hooks
export { useAppCollection } from "./hooks/useAppCollection";
export type { UseAppCollectionResult } from "./hooks/useAppCollection";

export { useAppRecord } from "./hooks/useAppRecord";
export type { UseAppRecordResult } from "./hooks/useAppRecord";

export { useRuntimeStatus } from "./hooks/useRuntimeStatus";
export type { UseRuntimeStatusResult } from "./hooks/useRuntimeStatus";

export { useAuth } from "./hooks/useAuth";
export type { UseAuthResult } from "./hooks/useAuth";

export { usePermissions } from "./hooks/usePermissions";
export type { UsePermissionsResult } from "./hooks/usePermissions";

// Components
export { PermissionsProvider, usePermissionsContext } from "./components/PermissionsProvider";
export type { PermissionsProviderProps } from "./components/PermissionsProvider";

export { Authorized } from "./components/Authorized";
export type { AuthorizedProps } from "./components/Authorized";

export { AuthGate } from "./components/AuthGate";
export type { AuthGateProps, AuthFormSlotProps } from "./components/AuthGate";
