import { listLlmModels } from "@/core/api";

export interface LlmModel {
  id: string;
  name: string;
  provider: string;
  model: string;
  config: any;
  is_default: boolean;
}

export interface AIProvider {
  id: string;
  name: string;
  env: string[];
}

export const AI_PROVIDERS: AIProvider[] = [
  { id: "rootcx", name: "RootCX (Managed)", env: ["ROOTCX_API_KEY"] },
  { id: "anthropic", name: "Anthropic", env: ["ANTHROPIC_API_KEY"] },
  { id: "openai", name: "OpenAI", env: ["OPENAI_API_KEY"] },
  { id: "bedrock", name: "AWS Bedrock", env: [] },
];

export type AwsAuthMode = "iam" | "apikey";

export const AWS_AUTH_MODES: Record<AwsAuthMode, { label: string; env: string[] }> = {
  apikey: { label: "API Key", env: ["AWS_BEARER_TOKEN_BEDROCK", "AWS_DEFAULT_REGION"] },
  iam: { label: "IAM Credentials", env: ["AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY", "AWS_DEFAULT_REGION"] },
};

export function envKeysForProvider(p: AIProvider, awsAuthMode?: AwsAuthMode): string[] {
  if (p.id === "bedrock" && awsAuthMode) return AWS_AUTH_MODES[awsAuthMode].env;
  return p.env;
}

const DEFAULT_MODELS: Record<string, string> = {
  rootcx: "rootcx",
  anthropic: "claude-sonnet-4-6",
  openai: "gpt-4.1",
  bedrock: "us.anthropic.claude-sonnet-4-6",
};

export function defaultModelForProvider(providerId: string): string {
  return DEFAULT_MODELS[providerId] ?? "claude-sonnet-4-6";
}

// Reactive store — fetches from Core's llm-models API
let models: LlmModel[] = [];
let loaded = false;
const listeners = new Set<() => void>();
function emit() { listeners.forEach((fn) => fn()); }

export const llmStore = {
  subscribe(fn: () => void) { listeners.add(fn); return () => listeners.delete(fn); },
  getSnapshot() { return models; },
  isLoaded() { return loaded; },
  getDefault() { return models.find((m) => m.is_default) ?? models[0] ?? null; },
  async refresh() {
    try {
      models = await listLlmModels();
      loaded = true;
    } catch {
      // runtime not ready
    }
    emit();
  },
};
