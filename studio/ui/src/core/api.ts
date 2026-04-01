import { fetchCore } from "./auth";
import type { LlmModel } from "@/lib/ai-models";
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

export const listSecrets = () => json<string[]>("/api/v1/platform/secrets");
export const setSecret = (key: string, value: string) => send("/api/v1/platform/secrets", post({ key, value }));
export const deleteSecret = (key: string) => send(`/api/v1/platform/secrets/${encodeURIComponent(key)}`, { method: "DELETE" });

export const listLlmModels = () => json<LlmModel[]>("/api/v1/llm-models");
export const createLlmModel = (model: Omit<LlmModel, "config"> & { config?: any }) =>
  json<LlmModel>("/api/v1/llm-models", post(model));
export const deleteLlmModel = (id: string) => send(`/api/v1/llm-models/${encodeURIComponent(id)}`, { method: "DELETE" });
export const setDefaultLlmModel = (id: string) => send(`/api/v1/llm-models/${encodeURIComponent(id)}/default`, { method: "PUT" });

export const verifySchema = (manifest: unknown) => json<SchemaVerification>("/api/v1/apps/schema/verify", post(manifest));
export const syncManifest = (manifest: unknown) => send("/api/v1/apps", post(manifest));
