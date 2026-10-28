export function publicErrorCode(error: any): string {
  for (let depth = 0; error?.cause && depth < 5; depth++) error = error.cause;
  const detail = error?.error?.error ?? error?.error;
  if ((error?.status === 402 || error?.constructor?.name === "APIError") && detail?.type === "credits_exhausted") return "AI_CREDITS_EXHAUSTED";
  if ((error?.status === 402 || error?.constructor?.name === "APIError") && detail?.type === "credits_reserved") return "AI_CREDITS_RESERVED";
  if (error?.status === 429) return "AI_RATE_LIMITED";
  if (error?.status >= 500 || ["APIConnectionError", "APIConnectionTimeoutError"].includes(error?.constructor?.name)) return "AI_UNAVAILABLE";
  return "AGENT_FAILED";
}
