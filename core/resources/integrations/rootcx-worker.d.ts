// Ambient type declarations for the RootCX worker prelude (v5).
// The prelude (`core/src/backend_prelude.js`) injects these globals at
// runtime via `--preload`. This file provides TypeScript with the shapes
// so integrations can call `serve()`, `log.*`, `emit()` etc. without
// per-file `declare const` hacks.

interface RootCxSqlResult {
  columns: string[];
  rows: unknown[][];
  rowCount: number;
}

interface RootCxTransaction {
  /** Run one statement in this transaction under the caller's governed identity. */
  sql(text: string, params?: unknown[]): Promise<RootCxSqlResult>;
}

interface RootCxStoredFile {
  fileId: string;
  appId: string;
  name: string;
  contentType: string;
  size: number;
}

interface RootCxBufferedFile extends RootCxStoredFile {
  content: Uint8Array;
}

interface RootCxStreamingFile extends RootCxStoredFile {
  stream: ReadableStream<Uint8Array>;
}

interface RootCxCollectionPage<T = unknown> {
  data: T[];
  total: number;
}

/** Explicit paging options for local and remote findPage; filters belong under where. */
interface RootCxFindPageOptions {
  /** Remote filter and sort fields must be in the grant's readable snapshot. */
  where?: Record<string, unknown>;
  orderBy?: string;
  order?: "asc" | "desc" | "ASC" | "DESC";
  /** Integer from 1 to 1000; defaults to 100. */
  limit?: number;
  /** Nonnegative integer; defaults to 0. */
  offset?: number;
}

/** Compatibility name for remote paging options; pass these to findPage, not find. */
type RootCxRemoteQuery = RootCxFindPageOptions;

/**
 * Shared local/remote collection API. T describes returned rows; for remote
 * collections it must match the approved projection, not the full provider entity.
 * Types neither validate responses nor confer permissions. Calls reject on Core
 * errors and cannot run inside ctx.transaction.
 */
interface RootCxCollection<T = unknown> {
  /** Full array of equality matches; {} reads all visible rows. Remote requires list. */
  find<R = T>(where?: Record<string, unknown>): Promise<R[]>;
  /** Explicit page, default limit 100. Remote requires list. */
  findPage<R = T>(options?: RootCxFindPageOptions): Promise<RootCxCollectionPage<R>>;
  /** Equality map only, including reserved option names. No visible match returns null. Remote requires read. */
  findOne<R = T>(where?: Record<string, unknown>): Promise<R | null>;
  /** Remote requires create and explicit writeFields approval for every payload field. */
  create<R = T>(data: Record<string, unknown>): Promise<R>;
  /** Alias for create, with the same grant action and response projection. */
  insert<R = T>(data: Record<string, unknown>): Promise<R>;
  /** Object form must include a UUID id alongside the changed fields. Remote requires update and writeFields. */
  update<R = T>(data: Record<string, unknown>): Promise<R>;
  /** Explicit UUID id plus changed fields. Remote requires update and writeFields. */
  update<R = T>(id: string, data: Record<string, unknown>): Promise<R>;
  /** UUID id; no visible match rejects. Remote requires delete. */
  delete(id: string): Promise<{ id: string; deleted: true }>;
}

/** Explicitly selected provider; each method owns a separate provider transaction. */
interface RootCxRemoteCollection<T = unknown> extends RootCxCollection<T> {}

interface RootCxRemoteApp {
  collection<T = unknown>(entity: string): RootCxRemoteCollection<T>;
}

interface RootCxCtx {
  readonly appId: string;
  readonly runtimeUrl: string;
  readonly credentials: Record<string, string>;
  readonly agentConfig: unknown;
  readonly log: typeof log;
  readonly emit: typeof emit;
  // Run SQL through Core under the caller's RLS identity, confined to this app.
  // Provider data requires remote(), even with a grant. Params are positional ($1, $2, …).
  sql(text: string, params?: unknown[]): Promise<RootCxSqlResult>;
  /**
   * Run database work atomically. The Core commits only when the callback and
   * every tx.sql call succeed; otherwise it rolls back. External capabilities
   * are intentionally unavailable inside the callback.
   */
  transaction<T>(callback: (tx: RootCxTransaction) => T | PromiseLike<T>): Promise<T>;
  // Privileged self-action over IPC (integrations) — no token replay.
  selfAction(action: string, params?: Record<string, unknown>): Promise<any>;
  // Invoke one of this app's own actions; credentials resolve for the caller.
  // Same-app sibling of callIntegration. Returns the action's raw result.
  // This is not the agent tool call_action and does not take a provider app ID.
  action(name: string, input?: Record<string, unknown>): Promise<any>;
  // Call another integration's action, gated by the (app x user) binding.
  callIntegration(integrationId: string, action: string, input?: Record<string, unknown>, asUser?: string): Promise<any>;
  uploadFile(content: string | Uint8Array, filename: string, contentType: string): Promise<string>;
  downloadFile(fileId: string): Promise<RootCxBufferedFile>;
  downloadFile(appId: string, fileId: string): Promise<RootCxBufferedFile>;
  openFile(fileId: string): Promise<RootCxStreamingFile>;
  openFile(appId: string, fileId: string): Promise<RootCxStreamingFile>;
  enqueueJob(payload: unknown): Promise<{ msgId: number }>;
  /** Same-app collection; optional T adds row typing while retaining legacy untyped calls. */
  collection<T = any>(entity: string): RootCxCollection<T>;
  /**
   * Access a provider collection through explicit Core grants and provider RLS.
   * Requires protocol v5 and an active principal; unavailable inside transaction().
   * Agent workers must use query_data/mutate_data through Core tool dispatch.
   * This handle cannot call provider actions or invoke agents.
   */
  remote(providerApp: string): RootCxRemoteApp;
}

interface RootCxServeHandlers {
  rpc?: Record<string, (params: any, caller: any, ctx: RootCxCtx) => Promise<any> | any>;
  onStart?: (ctx: RootCxCtx) => void | Promise<void>;
  onJob?: (payload: any, caller: any, ctx: RootCxCtx) => any | Promise<any>;
  onShutdown?: () => void | Promise<void>;
}

declare const serve: (handlers: RootCxServeHandlers) => void;

declare const log: {
  info(message: string): void;
  warn(message: string): void;
  error(message: string): void;
};

declare const emit: (name: string, data?: Record<string, unknown>) => void;

declare const uploadFile: (
  content: string | Uint8Array,
  filename: string,
  contentType: string,
) => Promise<string>;
