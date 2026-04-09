// Ambient type declarations for the RootCX worker prelude (v2).
// The prelude (`core/src/backend_prelude.js`) injects these globals at
// runtime via `--preload`. This file provides TypeScript with the shapes
// so integrations can call `serve()`, `log.*`, `emit()` etc. without
// per-file `declare const` hacks.

interface RootCxCtx {
  readonly appId: string;
  readonly runtimeUrl: string;
  readonly databaseUrl: string;
  readonly credentials: Record<string, string>;
  readonly agentConfig: unknown;
  readonly log: typeof log;
  readonly emit: typeof emit;
  uploadFile(content: string | Uint8Array, filename: string, contentType: string): Promise<string>;
  collection(entity: string): {
    insert(data: Record<string, unknown>): Promise<any>;
    update(data: Record<string, unknown>): Promise<any>;
  };
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
