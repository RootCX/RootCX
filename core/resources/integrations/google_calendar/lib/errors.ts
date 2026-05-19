export type CalendarErrorCode =
  | "INSUFFICIENT_PERMISSIONS"
  | "TEMPORARY_ERROR"
  | "SYNC_CURSOR_ERROR"
  | "NOT_FOUND"
  | "MISCONFIGURED"
  | "UNKNOWN";

export interface CalendarError {
  code: CalendarErrorCode;
  message: string;
  /** Unix ms when the next retry should be attempted (TEMPORARY_ERROR only). */
  retryAfter?: number;
}

export type Result<T> = { ok: true; data: T } | { ok: false; error: CalendarError };

export const ok = <T>(data: T): Result<T> => ({ ok: true, data });
export const fail = (error: CalendarError): Result<never> => ({ ok: false, error });

const RETRY_AFTER_RE = /Retry after (\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z)/i;

export function parseRetryAfter(message: string): number | undefined {
  const m = message.match(RETRY_AFTER_RE);
  if (!m) return undefined;
  const t = Date.parse(m[1]);
  if (!isFinite(t) || t <= Date.now()) return undefined;
  return t;
}

interface RawApiError {
  status?: number;
  reason?: string;
  message?: string;
}

function rawFromHttp(status: number, body: string): RawApiError {
  let reason: string | undefined;
  let message: string = body;
  try {
    const parsed = JSON.parse(body);
    const errs = parsed?.error?.errors;
    if (Array.isArray(errs) && errs.length > 0) {
      reason = errs[0].reason;
      message = errs[0].message ?? message;
    } else if (typeof parsed?.error === "string") {
      reason = parsed.error;
      message = parsed.error_description ?? message;
    } else if (parsed?.error?.message) {
      message = parsed.error.message;
      reason = parsed.error.status;
    }
  } catch { /* body is not JSON */ }
  return { status, reason, message };
}

const NETWORK_CODES = new Set(["ECONNABORTED", "ENOTFOUND", "ECONNRESET", "ETIMEDOUT", "ERR_NETWORK"]);

function classify(raw: RawApiError): CalendarError {
  const { status, reason, message = "Unknown error" } = raw;

  if (typeof status === "string" && NETWORK_CODES.has(status)) {
    return { code: "TEMPORARY_ERROR", message };
  }

  switch (status) {
    case 400:
      if (reason === "invalid_grant") return { code: "INSUFFICIENT_PERMISSIONS", message };
      return { code: "MISCONFIGURED", message };

    case 401:
      return { code: "INSUFFICIENT_PERMISSIONS", message };

    case 403: {
      if (reason === "rateLimitExceeded" || reason === "userRateLimitExceeded" || reason === "quotaExceeded") {
        return { code: "TEMPORARY_ERROR", message, retryAfter: parseRetryAfter(message) };
      }
      if (reason === "forbidden" || reason === "forbiddenForNonOrganizer") {
        return { code: "INSUFFICIENT_PERMISSIONS", message };
      }
      if (reason === "accessNotConfigured" || /has not been used in project|is disabled/i.test(message)) {
        return { code: "MISCONFIGURED", message };
      }
      return { code: "UNKNOWN", message };
    }

    case 404:
      return { code: "NOT_FOUND", message };

    case 410:
      if (reason === "fullSyncRequired" || reason === "updatedMinTooLongAgo") {
        return { code: "SYNC_CURSOR_ERROR", message };
      }
      return { code: "NOT_FOUND", message };

    case 429:
      return { code: "TEMPORARY_ERROR", message, retryAfter: parseRetryAfter(message) };

    case 500: case 502: case 504:
      if (reason === "backendError" || reason === "internal_failure") return { code: "TEMPORARY_ERROR", message };
      return { code: "UNKNOWN", message };

    case 503:
      return { code: "TEMPORARY_ERROR", message };

    default:
      return { code: "UNKNOWN", message };
  }
}

export function classifyHttp(status: number, body: string): CalendarError {
  return classify(rawFromHttp(status, body));
}

const BACKOFF_BASE_MS = 1000;
const BACKOFF_JITTER_MS = 250;
const MAX_ATTEMPTS = 4;

const sleep = (ms: number) => new Promise<void>(r => setTimeout(r, ms));

export async function withRetry<T>(fn: () => Promise<Result<T>>): Promise<Result<T>> {
  let last: Result<T> | null = null;
  for (let attempt = 0; attempt < MAX_ATTEMPTS; attempt++) {
    const r = await fn();
    if (r.ok || r.error.code !== "TEMPORARY_ERROR") return r;
    last = r;
    if (attempt === MAX_ATTEMPTS - 1) break;
    const backoff = BACKOFF_BASE_MS * 2 ** attempt + Math.floor(Math.random() * BACKOFF_JITTER_MS);
    const retryIn = r.error.retryAfter ? Math.max(0, r.error.retryAfter - Date.now()) : Infinity;
    await sleep(Math.min(retryIn, backoff));
  }
  return last!;
}
