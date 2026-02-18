/**
 * Low-level HTTP client for the RootCX Runtime daemon.
 *
 * All methods talk to `http://localhost:{port}/api/v1/...`.
 */

export interface RuntimeClientOptions {
  baseUrl?: string;
}

export interface OsStatus {
  runtime: { version: string; state: string };
  postgres: { state: string; port: number | null; data_dir: string | null };
  forge: { state: string; port: number | null };
}

export interface InstalledApp {
  id: string;
  name: string;
  version: string;
  status: string;
  entities: string[];
}

export interface AppManifest {
  appId: string;
  name: string;
  version?: string;
  description?: string;
  routes?: unknown[];
  permissions?: unknown[];
  dataContract?: unknown[];
}

export interface AuthUser {
  id: string;
  username: string;
  email: string | null;
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
  username: string;
  password: string;
  email?: string;
  displayName?: string;
}

export interface RpcCaller {
  userId: string;
  username: string;
}

const DEFAULT_BASE_URL = "http://localhost:9100";

export class RuntimeClient {
  private baseUrl: string;
  private accessToken: string | null = null;
  private refreshToken: string | null = null;

  constructor(opts?: RuntimeClientOptions) {
    this.baseUrl = opts?.baseUrl ?? DEFAULT_BASE_URL;
  }

  /** Set tokens (e.g. restored from localStorage). */
  setTokens(access: string | null, refresh: string | null): void {
    this.accessToken = access;
    this.refreshToken = refresh;
  }

  getAccessToken(): string | null {
    return this.accessToken;
  }

  getRefreshToken(): string | null {
    return this.refreshToken;
  }

  // ── Health ──────────────────────────────────────────

  async isAvailable(): Promise<boolean> {
    try {
      const res = await fetch(`${this.baseUrl}/health`);
      return res.ok;
    } catch {
      return false;
    }
  }

  async waitForReady(timeoutMs = 30000): Promise<void> {
    const start = Date.now();
    while (Date.now() - start < timeoutMs) {
      if (await this.isAvailable()) return;
      await new Promise((r) => setTimeout(r, 500));
    }
    throw new RuntimeApiError(0, `Runtime not ready after ${timeoutMs}ms`);
  }

  // ── Status ──────────────────────────────────────────

  async status(): Promise<OsStatus> {
    const res = await fetch(`${this.baseUrl}/api/v1/status`);
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  // ── Apps management ─────────────────────────────────

  async installApp(manifest: AppManifest): Promise<{ message: string }> {
    const res = await fetch(`${this.baseUrl}/api/v1/apps`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(manifest),
    });
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  async listApps(): Promise<InstalledApp[]> {
    const res = await fetch(`${this.baseUrl}/api/v1/apps`);
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  async uninstallApp(appId: string): Promise<{ message: string }> {
    const res = await fetch(`${this.baseUrl}/api/v1/apps/${encodeURIComponent(appId)}`, {
      method: "DELETE",
    });
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  // ── Collections CRUD ────────────────────────────────

  async listRecords<T = Record<string, unknown>>(
    appId: string,
    entity: string,
  ): Promise<T[]> {
    const url = `${this.baseUrl}/api/v1/apps/${enc(appId)}/collections/${enc(entity)}`;
    const res = await fetch(url);
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  async createRecord<T = Record<string, unknown>>(
    appId: string,
    entity: string,
    data: Record<string, unknown>,
  ): Promise<T> {
    const url = `${this.baseUrl}/api/v1/apps/${enc(appId)}/collections/${enc(entity)}`;
    const res = await fetch(url, {
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
    const res = await fetch(url);
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
    const res = await fetch(url, {
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
    const res = await fetch(url, { method: "DELETE" });
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  // ── RPC ────────────────────────────────────────────

  async rpc(appId: string, method: string, params?: Record<string, unknown>): Promise<unknown> {
    const res = await this.authFetch(`${this.baseUrl}/api/v1/apps/${enc(appId)}/rpc`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ method, params: params ?? {} }),
    });
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  // ── Auth ───────────────────────────────────────────

  async register(data: RegisterInput): Promise<{ user: AuthUser }> {
    const res = await fetch(`${this.baseUrl}/api/v1/auth/register`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(data),
    });
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  async login(username: string, password: string): Promise<LoginResponse> {
    const res = await fetch(`${this.baseUrl}/api/v1/auth/login`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ username, password }),
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
      } catch {
        // best-effort
      }
    }
    this.accessToken = null;
    this.refreshToken = null;
  }

  async me(): Promise<AuthUser> {
    const res = await this.authFetch(`${this.baseUrl}/api/v1/auth/me`);
    if (!res.ok) throw new RuntimeApiError(res.status, await res.text());
    return res.json();
  }

  /** Fetch with Authorization header and auto-refresh on 401. */
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
