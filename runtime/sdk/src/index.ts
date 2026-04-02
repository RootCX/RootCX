export { RuntimeClient, RuntimeApiError, DEFAULT_BASE_URL } from "./client";
export type {
  RuntimeClientOptions,
  AuthUser,
  AuthMode,
  OidcProvider,
  LoginResponse,
  RegisterInput,
  RoleDefinition,
  RoleAssignment,
  PermissionDeclaration,
  EffectivePermissions,
  WhereOperator,
  WhereValue,
  FieldCondition,
  WhereClause,
  QueryOptions,
  QueryResult,
  IntegrationSummary,
  ActionDefinition,
  IntegrationBinding,
  IdentityRecord,
  Job,
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

export { useIntegration } from "./hooks/useIntegration";
export type { UseIntegrationResult } from "./hooks/useIntegration";

export { useIdentity } from "./hooks/useIdentity";
export type { UseIdentityResult } from "./hooks/useIdentity";

export { PermissionsProvider, usePermissionsContext } from "./components/PermissionsProvider";
export type { PermissionsProviderProps } from "./components/PermissionsProvider";

export { Authorized } from "./components/Authorized";
export type { AuthorizedProps } from "./components/Authorized";

export { AuthGate } from "./components/AuthGate";
export type { AuthGateProps, AuthFormSlotProps } from "./components/AuthGate";
