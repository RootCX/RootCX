export interface RuntimeClientOptions {
  baseUrl?: string;
  /**
   * Initial access token. Pass a share token here for public-share sessions:
   * `new RuntimeClient({ accessToken: shareToken, persist: false, autoRefresh: false })`
   */
  accessToken?: string;
  /**
   * If false, the client signals that callers (e.g. RuntimeProvider) must NOT
   * persist its tokens to localStorage. Public-share sessions should set this
   * to false so the share token never lands in browser storage.
   * Default: true (existing behavior).
   */
  persist?: boolean;
  /**
   * If false, 401 responses are returned to the caller without attempting a
   * refresh-token round-trip. Required for share-token sessions where
   * `/auth/refresh` would reject anyway.
   * Default: true (existing behavior).
   */
  autoRefresh?: boolean;
}

/** Share creation response — token is only returned once at creation. */
export interface PublicShareInfo {
  id: string;
  url: string;
  /** Raw share token. Empty string when re-fetching an already-active share. */
  token: string;
  tokenPrefix: string;
  context: Record<string, unknown>;
  createdAt: string;
  revoked: boolean;
}

/** Share listing for the owner's UI. Never includes the raw token. */
export interface PublicShareListing {
  id: string;
  appId: string;
  context: Record<string, unknown>;
  tokenPrefix: string;
  createdAt: string;
  lastAccessedAt: string | null;
  accessCount: number;
}

/** Resolved share — what a `/share/:token` frontend needs to render itself. */
export interface PublicShareLookup {
  appId: string;
  context: Record<string, unknown>;
}

export interface AuthUser {
  id: string;
  email: string;
  displayName: string | null;
  createdAt: string;
}

export interface LoginResponse {
  accessToken: string;
  refreshToken: string;
  expiresIn: number;
  user: AuthUser;
}

export interface RegisterInput {
  email: string;
  password: string;
  displayName?: string;
}

export interface OidcProvider {
  id: string;
  displayName: string;
}

export interface AuthMode {
  authRequired: boolean;
  setupRequired: boolean;
  passwordLoginEnabled: boolean;
  providers: OidcProvider[];
}

export interface RoleDefinition {
  name: string;
  description: string | null;
  inherits: string[];
  permissions: string[];
}

export interface RoleAssignment {
  userId: string;
  role: string;
  assignedAt: string;
}

export interface PermissionDeclaration {
  key: string;
  description: string;
}

export interface EffectivePermissions {
  roles: string[];
  permissions: string[];
}

export interface Job {
  msg_id: number;
  app_id: string;
  payload: Record<string, unknown>;
  user_id: string | null;
  read_ct: number;
  enqueued_at: string;
}

export type OverlapPolicy = "skip" | "queue";

export interface CronSchedule {
  id: string;
  appId: string;
  name: string;
  schedule: string;
  timezone: string | null;
  payload: Record<string, unknown>;
  overlapPolicy: OverlapPolicy;
  enabled: boolean;
  pgJobId: number | null;
  createdBy: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface CreateCronInput {
  name: string;
  schedule: string;
  timezone?: string;
  payload?: Record<string, unknown>;
  overlapPolicy?: OverlapPolicy;
}

export interface UpdateCronInput {
  schedule?: string;
  payload?: Record<string, unknown>;
  overlapPolicy?: OverlapPolicy;
  enabled?: boolean;
}

export interface Webhook {
  id: string;
  name: string;
  method: string;
  token: string;
  url: string;
  createdAt: string;
}

export interface IntegrationSummary {
  id: string;
  name: string;
  version: string;
  description: string;
  actions: ActionDefinition[];
  configSchema: Record<string, unknown> | null;
  webhooks: string[];
}

export interface ActionDefinition {
  id: string;
  name: string;
  description: string;
  inputSchema?: Record<string, unknown>;
  outputSchema?: Record<string, unknown>;
}

export interface IntegrationBinding {
  integrationId: string;
  enabled: boolean;
  webhookToken: string | null;
  createdAt: string;
}

export type WhereOperator =
  | "$eq"
  | "$ne"
  | "$gt"
  | "$gte"
  | "$lt"
  | "$lte"
  | "$like"
  | "$ilike"
  | "$in"
  | "$nin"
  | "$contains"
  | "$isNull";

export type WhereValue = string | number | boolean | null | unknown[];

export type FieldCondition =
  | WhereValue
  | Partial<Record<WhereOperator, WhereValue>>;

export type WhereClause = {
  [field: string]: FieldCondition | WhereClause[] | WhereClause;
} & {
  $and?: WhereClause[];
  $or?: WhereClause[];
  $not?: WhereClause;
};

export interface QueryOptions {
  where?: WhereClause;
  orderBy?: string;
  order?: "asc" | "desc";
  limit?: number;
  offset?: number;
  linked?: boolean | string[];
}

export interface QueryResult<T> {
  data: T[];
  total: number;
}

export type IdentityRecord<T> = T & { _source: { app: string; entity: string } };

declare global {
  interface ImportMetaEnv { VITE_ROOTCX_URL?: string }
  interface ImportMeta { readonly env: ImportMetaEnv }
}

function resolveBaseUrl(): string {
  if (import.meta.env.VITE_ROOTCX_URL) return import.meta.env.VITE_ROOTCX_URL;
  if (typeof window !== "undefined" && window.location.hostname !== "localhost") {
    return window.location.origin;
  }
  return "http://localhost:9100";
}

export const DEFAULT_BASE_URL = resolveBaseUrl();

export class RuntimeClient {
  private baseUrl: string;
  private accessToken: string | null = null;
  private refreshToken: string | null = null;
  /** False for public-share sessions — consumers must not persist tokens. */
  public readonly persist: boolean;
  /** False for share-token sessions — never call /auth/refresh on 401. */
  public readonly autoRefresh: boolean;

  constructor(opts?: RuntimeClientOptions) {
    this.baseUrl = opts?.baseUrl ?? DEFAULT_BASE_URL;
    this.accessToken = opts?.accessToken ?? null;
    this.persist = opts?.persist ?? true;
    this.autoRefresh = opts?.autoRefresh ?? true;
  }

  setTokens(access: string | null, refresh: string | null): void {
    this.accessToken = access;
    this.refreshToken = refresh;
  }

  getBaseUrl(): string {
    return this.baseUrl;
  }

  getAccessToken(): string | null {
    return this.accessToken;
  }

  getRefreshToken(): string | null {
    return this.refreshToken;
  }

  async listRecords<T = Record<string, unknown>>(
    appId: string,
    entity: string,
    params?: Record<string, string>,
  ): Promise<T[]> {
    const base = `${this.baseUrl}/api/v1/apps/${enc(appId)}/collections/${enc(entity)}`;
    const qs = params ? "?" + new URLSearchParams(params).toString() : "";
    const res = await this.authFetch(base + qs);
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  async queryRecords<T = Record<string, unknown>>(
    appId: string,
    entity: string,
    opts: QueryOptions,
  ): Promise<QueryResult<T>> {
    const url = `${this.baseUrl}/api/v1/apps/${enc(appId)}/collections/${enc(entity)}/query`;
    const res = await this.authFetch(url, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(opts),
    });
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  async identityQuery<T = Record<string, unknown>>(
    identityKind: string,
    opts?: QueryOptions,
  ): Promise<QueryResult<IdentityRecord<T>>> {
    const url = `${this.baseUrl}/api/v1/federated/${enc(identityKind)}/query`;
    const res = await this.authFetch(url, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(opts ?? {}),
    });
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  async createRecord<T = Record<string, unknown>>(
    appId: string,
    entity: string,
    data: Record<string, unknown>,
  ): Promise<T> {
    const url = `${this.baseUrl}/api/v1/apps/${enc(appId)}/collections/${enc(entity)}`;
    const res = await this.authFetch(url, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(data),
    });
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  async bulkCreateRecords<T = Record<string, unknown>>(
    appId: string,
    entity: string,
    data: Record<string, unknown>[],
  ): Promise<T[]> {
    const url = `${this.baseUrl}/api/v1/apps/${enc(appId)}/collections/${enc(entity)}/bulk`;
    const res = await this.authFetch(url, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(data),
    });
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  async getRecord<T = Record<string, unknown>>(
    appId: string,
    entity: string,
    id: string,
  ): Promise<T> {
    const url = `${this.baseUrl}/api/v1/apps/${enc(appId)}/collections/${enc(entity)}/${enc(id)}`;
    const res = await this.authFetch(url);
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  async updateRecord<T = Record<string, unknown>>(
    appId: string,
    entity: string,
    id: string,
    data: Record<string, unknown>,
  ): Promise<T> {
    const url = `${this.baseUrl}/api/v1/apps/${enc(appId)}/collections/${enc(entity)}/${enc(id)}`;
    const res = await this.authFetch(url, {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(data),
    });
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  async deleteRecord(
    appId: string,
    entity: string,
    id: string,
  ): Promise<{ message: string }> {
    const url = `${this.baseUrl}/api/v1/apps/${enc(appId)}/collections/${enc(entity)}/${enc(id)}`;
    const res = await this.authFetch(url, { method: "DELETE" });
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  async rpc(appId: string, method: string, params?: Record<string, unknown>): Promise<unknown> {
    const res = await this.authFetch(`${this.baseUrl}/api/v1/apps/${enc(appId)}/rpc`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ method, params: params ?? {} }),
    });
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  async authMode(): Promise<AuthMode> {
    const res = await fetch(`${this.baseUrl}/api/v1/auth/mode`);
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  async oidcLogin(providerId: string): Promise<void> {
    // Tauri: open system browser via oidc_login command (RFC 8252)
    // Uses window.__TAURI__ public API (requires withGlobalTauri: true in tauri.conf.json)
    const tauri = typeof window !== "undefined" && (window as any).__TAURI__;
    if (tauri?.core?.invoke) {
      const tokens: { accessToken: string; refreshToken: string } = await tauri.core.invoke("oidc_login", { providerId });
      this.accessToken = tokens.accessToken;
      this.refreshToken = tokens.refreshToken;
      return;
    }
    // Browser: standard redirect
    const redirectUri = window.location.href.split("?")[0];
    window.location.href = `${this.baseUrl}/api/v1/auth/oidc/${encodeURIComponent(providerId)}/authorize?redirect_uri=${encodeURIComponent(redirectUri)}`;
  }

  async register(data: RegisterInput): Promise<{ user: AuthUser }> {
    const res = await fetch(`${this.baseUrl}/api/v1/auth/register`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(data),
    });
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  async login(email: string, password: string): Promise<LoginResponse> {
    const res = await fetch(`${this.baseUrl}/api/v1/auth/login`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ email, password }),
    });
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    const data: LoginResponse = await res.json();
    this.accessToken = data.accessToken;
    this.refreshToken = data.refreshToken;
    return data;
  }

  async refresh(): Promise<void> {
    if (!this.refreshToken) throw new RuntimeApiError(0, "no refresh token");
    const res = await fetch(`${this.baseUrl}/api/v1/auth/refresh`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ refreshToken: this.refreshToken }),
    });
    if (!res.ok) {
      this.accessToken = null;
      this.refreshToken = null;
      throw new RuntimeApiError(res.status, await res.text());
    }
    const data = await res.json();
    this.accessToken = data.accessToken;
  }

  async logout(): Promise<void> {
    if (this.refreshToken) {
      try {
        await fetch(`${this.baseUrl}/api/v1/auth/logout`, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ refreshToken: this.refreshToken }),
        });
      } catch { /* ignore */ }
    }
    this.accessToken = null;
    this.refreshToken = null;
  }

  async me(): Promise<AuthUser> {
    const res = await this.authFetch(`${this.baseUrl}/api/v1/auth/me`);
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  async listRoles(): Promise<RoleDefinition[]> {
    const res = await this.authFetch(`${this.baseUrl}/api/v1/roles`);
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  async listRoleAssignments(): Promise<RoleAssignment[]> {
    const res = await this.authFetch(`${this.baseUrl}/api/v1/roles/assignments`);
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  async assignRole(userId: string, role: string): Promise<{ message: string }> {
    const res = await this.authFetch(`${this.baseUrl}/api/v1/roles/assign`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ userId, role }),
    });
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  async revokeRole(userId: string, role: string): Promise<{ message: string }> {
    const res = await this.authFetch(`${this.baseUrl}/api/v1/roles/revoke`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ userId, role }),
    });
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  async getPermissions(userId?: string): Promise<EffectivePermissions> {
    const path = userId
      ? `/api/v1/permissions/${enc(userId)}`
      : `/api/v1/permissions`;
    const res = await this.authFetch(`${this.baseUrl}${path}`);
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  async getAvailablePermissions(): Promise<PermissionDeclaration[]> {
    const res = await this.authFetch(`${this.baseUrl}/api/v1/permissions/available`);
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  async createRole(
    data: { name: string; description?: string; inherits?: string[]; permissions?: string[] },
  ): Promise<{ message: string }> {
    const res = await this.authFetch(`${this.baseUrl}/api/v1/roles`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(data),
    });
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  async updateRole(
    roleName: string,
    data: { description?: string; inherits?: string[]; permissions?: string[] },
  ): Promise<{ message: string }> {
    const res = await this.authFetch(
      `${this.baseUrl}/api/v1/roles/${enc(roleName)}`,
      {
        method: "PATCH",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(data),
      },
    );
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  async deleteRole(roleName: string): Promise<{ message: string }> {
    const res = await this.authFetch(
      `${this.baseUrl}/api/v1/roles/${enc(roleName)}`,
      { method: "DELETE" },
    );
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  async listIntegrations(): Promise<IntegrationSummary[]> {
    const res = await this.authFetch(`${this.baseUrl}/api/v1/integrations`);
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  async listIntegrationBindings(appId: string): Promise<IntegrationBinding[]> {
    const res = await this.authFetch(
      `${this.baseUrl}/api/v1/apps/${enc(appId)}/integrations`,
    );
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  async bindIntegration(
    appId: string,
    integrationId: string,
    config?: Record<string, unknown>,
  ): Promise<{ message: string; webhookToken: string }> {
    const res = await this.authFetch(
      `${this.baseUrl}/api/v1/apps/${enc(appId)}/integrations`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ integrationId, config }),
      },
    );
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  async updateIntegrationConfig(
    appId: string,
    integrationId: string,
    config: Record<string, unknown>,
  ): Promise<{ message: string }> {
    const res = await this.authFetch(
      `${this.baseUrl}/api/v1/apps/${enc(appId)}/integrations/${enc(integrationId)}`,
      {
        method: "PATCH",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ config }),
      },
    );
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  async unbindIntegration(
    appId: string,
    integrationId: string,
  ): Promise<{ message: string }> {
    const res = await this.authFetch(
      `${this.baseUrl}/api/v1/apps/${enc(appId)}/integrations/${enc(integrationId)}`,
      { method: "DELETE" },
    );
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  async integrationAuthStatus(
    integrationId: string,
  ): Promise<{ connected: boolean }> {
    const res = await this.authFetch(
      `${this.baseUrl}/api/v1/integrations/${enc(integrationId)}/auth`,
    );
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  async integrationAuthStart(
    integrationId: string,
  ): Promise<{ type: string; url?: string; [key: string]: unknown }> {
    const res = await this.authFetch(
      `${this.baseUrl}/api/v1/integrations/${enc(integrationId)}/auth/start`,
      { method: "POST" },
    );
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  async integrationAuthSubmit(
    integrationId: string,
    credentials: Record<string, string>,
  ): Promise<{ message: string }> {
    const res = await this.authFetch(
      `${this.baseUrl}/api/v1/integrations/${enc(integrationId)}/auth/credentials`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ credentials }),
      },
    );
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  async integrationAuthDisconnect(
    integrationId: string,
  ): Promise<{ message: string }> {
    const res = await this.authFetch(
      `${this.baseUrl}/api/v1/integrations/${enc(integrationId)}/auth`,
      { method: "DELETE" },
    );
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  async enqueueJob(
    appId: string,
    payload?: Record<string, unknown>,
  ): Promise<{ msg_id: number }> {
    const res = await this.authFetch(`${this.baseUrl}/api/v1/apps/${enc(appId)}/jobs`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ payload: payload ?? {} }),
    });
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  async listJobs(appId: string, opts?: { archived?: boolean; limit?: number }): Promise<Job[]> {
    const params = new URLSearchParams();
    if (opts?.archived) params.set("archived", "true");
    if (opts?.limit) params.set("limit", String(opts.limit));
    const qs = params.toString() ? `?${params}` : "";
    const res = await this.authFetch(
      `${this.baseUrl}/api/v1/apps/${enc(appId)}/jobs${qs}`,
    );
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  async listCrons(appId: string): Promise<CronSchedule[]> {
    const res = await this.authFetch(`${this.baseUrl}/api/v1/apps/${enc(appId)}/crons`);
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  async listWebhooks(appId: string): Promise<Webhook[]> {
    const res = await this.authFetch(`${this.baseUrl}/api/v1/apps/${enc(appId)}/webhooks`);
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  async createCron(appId: string, input: CreateCronInput): Promise<CronSchedule> {
    const res = await this.authFetch(`${this.baseUrl}/api/v1/apps/${enc(appId)}/crons`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(input),
    });
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  async updateCron(appId: string, id: string, patch: UpdateCronInput): Promise<CronSchedule> {
    const res = await this.authFetch(`${this.baseUrl}/api/v1/apps/${enc(appId)}/crons/${enc(id)}`, {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(patch),
    });
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  async deleteCron(appId: string, id: string): Promise<void> {
    const res = await this.authFetch(`${this.baseUrl}/api/v1/apps/${enc(appId)}/crons/${enc(id)}`, {
      method: "DELETE",
    });
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
  }

  async triggerCron(appId: string, id: string): Promise<{ msgId: number }> {
    const res = await this.authFetch(`${this.baseUrl}/api/v1/apps/${enc(appId)}/crons/${enc(id)}/trigger`, {
      method: "POST",
    });
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  async callIntegration(
    integrationId: string,
    action: string,
    input?: Record<string, unknown>,
  ): Promise<unknown> {
    const res = await this.authFetch(
      `${this.baseUrl}/api/v1/integrations/${enc(integrationId)}/actions/${enc(action)}`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(input ?? {}),
      },
    );
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  /**
   * Create a public share for the given app. `context` is an opaque payload
   * that the app's public RPC handlers can read (via `identity.share_context`
   * server-side). The runtime enforces scope match against `context` when the
   * manifest's `public.rpcs[].scope` lists keys for the called RPC.
   *
   * The raw token is returned **once**. Store it in the caller's clipboard /
   * share modal; the server only retains the SHA-256 hash. If the same caller
   * already has an active share for the same context, the existing record is
   * returned with an empty `token` field — revoke and recreate to mint a new
   * token.
   */
  async createPublicShare(
    appId: string,
    opts: { context: Record<string, unknown> },
  ): Promise<PublicShareInfo> {
    const res = await this.authFetch(
      `${this.baseUrl}/api/v1/apps/${enc(appId)}/public-shares`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ context: opts.context }),
      },
    );
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  /** List the caller's active shares for the given app. Never returns raw tokens. */
  async listPublicShares(appId: string): Promise<PublicShareListing[]> {
    const res = await this.authFetch(
      `${this.baseUrl}/api/v1/apps/${enc(appId)}/public-shares`,
    );
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  /** Revoke a share by id. Filtered by `created_by` server-side — non-owners get 404. */
  async revokePublicShare(appId: string, shareId: string): Promise<{ message: string }> {
    const res = await this.authFetch(
      `${this.baseUrl}/api/v1/apps/${enc(appId)}/public-shares/${enc(shareId)}`,
      { method: "DELETE" },
    );
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  /**
   * Look up the share this client's accessToken belongs to. Only meaningful
   * when the client was constructed with a share token in `accessToken`.
   * Used by `/share/:token` frontends to discover the app and context the
   * share grants access to.
   */
  async getPublicShareInfo(): Promise<PublicShareLookup> {
    const res = await this.authFetch(`${this.baseUrl}/api/v1/public/share/info`);
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  core(): CoreNamespace {
    return new CoreNamespace(this);
  }

  /** @internal — exposed for CoreCollection */
  async fetchJson<T>(url: string, init?: RequestInit): Promise<T> {
    const res = await this.authFetch(url, init);
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  private async authFetch(url: string, init?: RequestInit): Promise<Response> {
    const doFetch = (token: string | null) => {
      const headers = new Headers(init?.headers);
      if (token) headers.set("Authorization", `Bearer ${token}`);
      return fetch(url, { ...init, headers });
    };

    let res = await doFetch(this.accessToken);
    if (res.status === 401 && this.refreshToken && this.autoRefresh) {
      try {
        await this.refresh();
        res = await doFetch(this.accessToken);
      } catch {
        // refresh failed, return original 401
      }
    }
    return res;
  }
}

function enc(s: string): string {
  return encodeURIComponent(s);
}

const CORE_ROUTES: Record<string, string> = {
  users: "/api/v1/users",
};

class CoreNamespace {
  constructor(private client: RuntimeClient) {}

  collection<T = Record<string, unknown>>(entity: string): CoreCollection<T> {
    const route = CORE_ROUTES[entity];
    if (!route) throw new Error(`unknown core entity: '${entity}'`);
    return new CoreCollection<T>(this.client, route);
  }
}

class CoreCollection<T = Record<string, unknown>> {
  constructor(private client: RuntimeClient, private base: string) {}

  list(): Promise<T[]> {
    return this.client.fetchJson(`${this.client.getBaseUrl()}${this.base}`);
  }

  get(id: string): Promise<T> {
    return this.client.fetchJson(`${this.client.getBaseUrl()}${this.base}/${enc(id)}`);
  }
}

export class RuntimeApiError extends Error {
  constructor(
    public readonly status: number,
    public readonly body: string,
  ) {
    super(`Runtime API error (${status}): ${body}`);
    this.name = "RuntimeApiError";
  }
}
