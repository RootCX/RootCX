export interface RuntimeClientOptions {
  baseUrl?: string;
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

  constructor(opts?: RuntimeClientOptions) {
    this.baseUrl = opts?.baseUrl ?? DEFAULT_BASE_URL;
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

  private async authFetch(url: string, init?: RequestInit): Promise<Response> {
    const doFetch = (token: string | null) => {
      const headers = new Headers(init?.headers);
      if (token) headers.set("Authorization", `Bearer ${token}`);
      return fetch(url, { ...init, headers });
    };

    let res = await doFetch(this.accessToken);
    if (res.status === 401 && this.refreshToken) {
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

export class RuntimeApiError extends Error {
  constructor(
    public readonly status: number,
    public readonly body: string,
  ) {
    super(`Runtime API error (${status}): ${body}`);
    this.name = "RuntimeApiError";
  }
}
