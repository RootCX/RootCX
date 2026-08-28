interface RootCxSqlResult {
  columns: string[];
  rows: unknown[][];
  rowCount: number;
}

interface RootCxTransaction {
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

interface RootCxCtx {
  readonly appId: string;
  readonly runtimeUrl: string;
  readonly credentials: Record<string, string>;
  readonly agentConfig: unknown;
  readonly log: typeof log;
  readonly emit: typeof emit;
  sql(text: string, params?: unknown[]): Promise<RootCxSqlResult>;
  transaction<T>(callback: (tx: RootCxTransaction) => T | PromiseLike<T>): Promise<T>;
  selfAction(action: string, params?: Record<string, unknown>): Promise<unknown>;
  action(name: string, input?: Record<string, unknown>): Promise<unknown>;
  callIntegration(
    integrationId: string,
    action: string,
    input?: Record<string, unknown>,
    asUser?: string,
  ): Promise<unknown>;
  uploadFile(content: string | Uint8Array, filename: string, contentType: string): Promise<string>;
  downloadFile(fileId: string): Promise<RootCxBufferedFile>;
  downloadFile(appId: string, fileId: string): Promise<RootCxBufferedFile>;
  openFile(fileId: string): Promise<RootCxStreamingFile>;
  openFile(appId: string, fileId: string): Promise<RootCxStreamingFile>;
  enqueueJob(payload: unknown): Promise<{ msgId: number }>;
  collection(entity: string): {
    insert(data: Record<string, unknown>): Promise<unknown>;
    update(data: Record<string, unknown>): Promise<unknown>;
    find(where?: Record<string, unknown>): Promise<unknown[]>;
    findOne(where?: Record<string, unknown>): Promise<unknown | null>;
  };
}

interface RootCxServeHandlers {
  rpc?: Record<string, (params: unknown, caller: unknown, ctx: RootCxCtx) => unknown | Promise<unknown>>;
  onStart?: (ctx: RootCxCtx) => void | Promise<void>;
  onJob?: (payload: unknown, caller: unknown, ctx: RootCxCtx) => unknown | Promise<unknown>;
  onShutdown?: () => void | Promise<void>;
}

declare const serve: (handlers: RootCxServeHandlers) => void;
declare const log: {
  info(message: string): void;
  warn(message: string): void;
  error(message: string): void;
};
declare const emit: (name: string, data?: Record<string, unknown>) => void;
