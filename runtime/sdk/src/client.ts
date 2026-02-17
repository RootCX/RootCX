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

const DEFAULT_BASE_URL = "http://localhost:9100";

export class RuntimeClient {
  private baseUrl: string;

  constructor(opts?: RuntimeClientOptions) {
    this.baseUrl = opts?.baseUrl ?? DEFAULT_BASE_URL;
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
