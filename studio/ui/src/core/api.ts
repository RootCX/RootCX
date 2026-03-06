import { fetchCore } from "./auth";
import type { AiConfig } from "@/lib/ai-models";
import type { SchemaVerification } from "@/types";

async function json<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetchCore(path, init);
  if (!res.ok) throw new Error(await res.text().catch(() => `${init?.method ?? "GET"} ${path} failed`));
  return res.json();
}

async function send(path: string, init?: RequestInit): Promise<void> {
  const res = await fetchCore(path, init);
  if (!res.ok) throw new Error(await res.text().catch(() => `${init?.method ?? "GET"} ${path} failed`));
}

const post = (body: unknown): RequestInit => ({
  method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body),
});

const put = (body: unknown): RequestInit => ({
  method: "PUT", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body),
});

export const listSecrets = () => json<string[]>("/api/v1/platform/secrets");
export const setSecret = (key: string, value: string) => send("/api/v1/platform/secrets", post({ key, value }));
export const deleteSecret = (key: string) => send(`/api/v1/platform/secrets/${encodeURIComponent(key)}`, { method: "DELETE" });

export const getAiConfig = () => json<AiConfig | null>("/api/v1/config/ai");
export const saveAiConfig = (config: AiConfig) => send("/api/v1/config/ai", put(config));

export const verifySchema = (manifest: unknown) => json<SchemaVerification>("/api/v1/apps/schema/verify", post(manifest));
export const syncManifest = (manifest: unknown) => send("/api/v1/apps", post(manifest));
