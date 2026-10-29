export function publicErrorCode(error: any): string {
  // Agent middleware wraps a provider error once per layer; classify the original one.
  for (const seen = new Set(); error?.cause && !seen.has(error.cause) && seen.size < 64; error = error.cause) seen.add(error);
  const detail = error?.error?.error ?? error?.error;
  if ((error?.status === 402 || error?.constructor?.name === "APIError") && detail?.type === "credits_exhausted") return "AI_CREDITS_EXHAUSTED";
  if ((error?.status === 402 || error?.constructor?.name === "APIError") && detail?.type === "credits_reserved") return "AI_CREDITS_RESERVED";
  if (error?.status === 429) return "AI_RATE_LIMITED";
  if (error?.status >= 500 || ["APIConnectionError", "APIConnectionTimeoutError"].includes(error?.constructor?.name)) return "AI_UNAVAILABLE";
  return "AGENT_FAILED";
}
