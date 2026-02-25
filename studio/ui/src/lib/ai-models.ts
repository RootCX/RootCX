import { getAiConfig } from "@/core/api";

const DEFAULT_MODEL = "claude-sonnet-4-6";

export interface AIProvider {
  id: string;
  name: string;
  env: string[];
}

export const AI_PROVIDERS: AIProvider[] = [
  { id: "anthropic", name: "Anthropic", env: ["ANTHROPIC_API_KEY"] },
  { id: "openai", name: "OpenAI", env: ["OPENAI_API_KEY"] },
  { id: "bedrock", name: "AWS Bedrock", env: [] },
];

export type AwsAuthMode = "iam" | "apikey";

export const AWS_AUTH_MODES: Record<AwsAuthMode, { label: string; env: string[] }> = {
  apikey: { label: "API Key", env: ["AWS_BEARER_TOKEN_BEDROCK"] },
  iam: { label: "IAM Credentials", env: ["AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY"] },
};

export function envKeysForProvider(p: AIProvider, awsAuthMode?: AwsAuthMode): string[] {
  if (p.id === "bedrock" && awsAuthMode) return AWS_AUTH_MODES[awsAuthMode].env;
  return p.env;
}

export interface AiConfig {
  provider: string;
  model: string;
  region?: string;
}

const DEFAULT_MODELS: Record<string, string> = {
  anthropic: DEFAULT_MODEL,
  openai: "o3",
  bedrock: DEFAULT_MODEL,
};

export function defaultAiConfig(providerId: string): AiConfig {
  return {
    provider: providerId,
    model: DEFAULT_MODELS[providerId] ?? DEFAULT_MODEL,
    ...(providerId === "bedrock" ? { region: "us-east-1" } : {}),
  };
}

// Reactive store — single source of truth from Core
let current: AiConfig | null = null;
let loaded = false;
const listeners = new Set<() => void>();
function emit() { listeners.forEach((fn) => fn()); }

export const aiConfigStore = {
  subscribe(fn: () => void) { listeners.add(fn); return () => listeners.delete(fn); },
  getSnapshot() { return current; },
  isLoaded() { return loaded; },
  async refresh() {
    try {
      current = await getAiConfig();
      loaded = true;
    } catch {
      // runtime not ready — don't update state
    }
    emit();
  },

  providerName() {
    if (!current) return null;
    return AI_PROVIDERS.find((p) => p.id === current!.provider)?.name ?? current.provider;
  },
};
