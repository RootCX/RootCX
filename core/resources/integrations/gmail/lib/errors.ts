export type GmailErrorCode =
  | "INSUFFICIENT_PERMISSIONS"
  | "TEMPORARY_ERROR"
  | "SYNC_CURSOR_ERROR"
  | "NOT_FOUND"
  | "MISCONFIGURED"
  | "UNKNOWN";

export interface GmailError {
  code: GmailErrorCode;
  message: string;
  /** Unix ms when the next retry should be attempted (TEMPORARY_ERROR only). */
  retryAfter?: number;
}

export type Result<T> = { ok: true; data: T } | { ok: false; error: GmailError };

export const ok = <T>(data: T): Result<T> => ({ ok: true, data });
export const fail = (error: GmailError): Result<never> => ({ ok: false, error });

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
  path?: string;
}

function rawFromHttp(status: number, body: string, path?: string): RawApiError {
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
  return { status, reason, message, path };
}

function rawFromGaxios(err: any): RawApiError {
  const status = err?.response?.status ?? err?.code;
  const data = err?.response?.data;
  const reason =
    data?.error?.errors?.[0]?.reason ??
    (typeof data?.error === "string" ? data.error : data?.error?.status);
  const message =
    data?.error?.errors?.[0]?.message ??
    data?.error_description ??
    data?.error?.message ??
    err?.message ??
    "Unknown error";
  const path = err?.config?.url;
  return { status, reason, message, path };
}

const NETWORK_CODES = new Set(["ECONNABORTED", "ENOTFOUND", "ECONNRESET", "ETIMEDOUT", "ERR_NETWORK"]);

function classify(raw: RawApiError): GmailError {
  const { status, reason, message = "Unknown error", path = "" } = raw;
  const onHistory = path.includes("/history");

  if (typeof status === "string" && NETWORK_CODES.has(status)) {
    return { code: "TEMPORARY_ERROR", message };
  }

  switch (status) {
    case 400:
      if (reason === "invalid_grant") return { code: "INSUFFICIENT_PERMISSIONS", message };
      if (reason === "failedPrecondition") {
        if (/mail service not enabled/i.test(message)) return { code: "INSUFFICIENT_PERMISSIONS", message };
        return { code: "TEMPORARY_ERROR", message };
      }
      return { code: "UNKNOWN", message };

    case 401:
      return { code: "INSUFFICIENT_PERMISSIONS", message };

    case 403: {
      if (reason === "rateLimitExceeded" || reason === "userRateLimitExceeded" || reason === "dailyLimitExceeded") {
        return { code: "TEMPORARY_ERROR", message, retryAfter: parseRetryAfter(message) };
      }
      if (reason === "domainPolicy" || reason === "insufficientPermissions") {
        return { code: "INSUFFICIENT_PERMISSIONS", message };
      }
      return { code: "UNKNOWN", message };
    }

    case 404:
      return { code: onHistory ? "SYNC_CURSOR_ERROR" : "NOT_FOUND", message };

    case 429:
      return { code: "TEMPORARY_ERROR", message, retryAfter: parseRetryAfter(message) };

    case 500: case 502: case 504:
      if (reason === "backendError" || reason === "internal_failure") return { code: "TEMPORARY_ERROR", message };
      if (/authentication backend unavailable/i.test(message)) return { code: "TEMPORARY_ERROR", message };
      return { code: "UNKNOWN", message };

    case 503:
      return { code: "TEMPORARY_ERROR", message };

    default:
      return { code: "UNKNOWN", message };
  }
}

/** Map a raw HTTP response (after fetch) to a typed error. Path informs 404 disambiguation. */
export function classifyHttp(status: number, body: string, path?: string): GmailError {
  return classify(rawFromHttp(status, body, path));
}

/** Map a googleapis-thrown error (Gaxios) to a typed error. */
export function classifyGaxios(err: any): GmailError {
  return classify(rawFromGaxios(err));
}

const BACKOFF_BASE_MS = 1000;
const BACKOFF_JITTER_MS = 250;
const MAX_ATTEMPTS = 4;

let _sleep = (ms: number) => new Promise<void>(r => setTimeout(r, ms));

/** Override the sleep function (for testing). */
export function _setSleep(fn: (ms: number) => Promise<void>): void { _sleep = fn; }

/** Retry only TEMPORARY_ERROR up to 3 retries (4 attempts total) with 1s/2s/4s + jitter. */
export async function withRetry<T>(fn: () => Promise<Result<T>>): Promise<Result<T>> {
  let last: Result<T> | null = null;
  for (let attempt = 0; attempt < MAX_ATTEMPTS; attempt++) {
    const r = await fn();
    if (r.ok || r.error.code !== "TEMPORARY_ERROR") return r;
    last = r;
    if (attempt === MAX_ATTEMPTS - 1) break;
    const backoff = BACKOFF_BASE_MS * 2 ** attempt + Math.floor(Math.random() * BACKOFF_JITTER_MS);
    const retryIn = r.error.retryAfter ? Math.max(0, r.error.retryAfter - Date.now()) : Infinity;
    await _sleep(Math.min(retryIn, backoff));
  }
  return last!;
}
