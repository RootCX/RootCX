// RootCX worker runtime prelude
// =============================
//
// Injected into every Bun worker spawned by the Core (`--preload`). Defines
// the IPC contract between a worker process and the Core supervisor. The
// contract is versioned; this file is the canonical reference for the JS
// side. The Rust side lives in `core/src/ipc.rs` — keep them in sync.
//
// ──────────────── Architecture ────────────────
//
//   ┌─────────────────────────┐
//   │  App code               │  ← knows only serve() + ctx
//   │  serve({ rpc, … })      │
//   ├─────────────────────────┤
//   │  Runtime (this file)    │  ← ctx factory, dispatch, pending maps
//   │  talks to Transport     │
//   ├─────────────────────────┤
//   │  Transport (pluggable)  │  ← today: JsonLinesStdioTransport
//   │  { send(msg),           │     tomorrow: gRPC, Unix socket, …
//   │    onMessage(dispatch) }│
//   └─────────────────────────┘
//
//   The runtime NEVER touches process.stdin/stdout directly. All I/O goes
//   through _transport.send() and _transport.onMessage(). To swap the wire
//   format, replace _createTransport() — nothing else changes.
//
// ──────────────── Protocol versions ────────────────
//
//   v1 (legacy) — pre-prelude apps that manage `process.stdin` themselves,
//     hand-write JSON-lines, and never call `serve()`. They continue to
//     work unchanged because this prelude stays silent on `discover` /
//     `rpc` / `job` / `shutdown` when no handlers are registered (see the
//     `if (!_handlers) return` branches below). Supported indefinitely:
//     existing client apps in production are v1.
//
//   v2 — apps call `globalThis.serve({ rpc, onStart, onJob, onShutdown })`.
//     The prelude owns a SINGLE stdin dispatcher and hands a `ctx` object
//     to every handler. `ctx` exposes the full worker capability surface:
//
//       ctx.appId, ctx.runtimeUrl,
//       ctx.credentials, ctx.agentConfig,
//       ctx.log, ctx.emit,
//       ctx.sql(text, params) → { columns, rows, rowCount },
//       ctx.selfAction(action, params),
//       ctx.uploadFile(content, filename, contentType),
//       ctx.collection(entity).insert(data) / .update / .find / .findOne,
//
//     A v2 worker MUST announce its version in the `discover` reply:
//
//       { "type": "discover", "protocol": 2, "methods": [ ... ] }
//
// ──────────────── Evolution rules ────────────────
//
//   * Adding an OPTIONAL field to an existing outbound message → no bump.
//   * Adding a new outbound message type (app → Core) → bump.
//   * Changing the semantics of an existing message → bump.
//   * Removing any message → bump + deprecation plan (never silent drop).
//
//   A new version MUST be additive w.r.t. legacy: the prelude code paths
//   for older versions stay compiled in. The Core decides per-worker what
//   to send based on the negotiated version (see `worker_protocol` in
//   `worker.rs`).

const PROTOCOL_VERSION = 2;

// ─── Transport layer ─────────────────────────────────────────────────────────
// Encapsulates the wire format and I/O channel. The rest of the prelude
// talks exclusively to _transport — never to process.stdin/stdout.

function _createJsonLinesTransport() {
  return {
    send(msg) {
      process.stdout.write(JSON.stringify(msg) + "\n");
    },
    onMessage(dispatch) {
      let buffer = "";
      process.stdin.setEncoding("utf-8");
      process.stdin.on("data", (chunk) => {
        buffer += chunk;
        let nl;
        while ((nl = buffer.indexOf("\n")) !== -1) {
          const line = buffer.slice(0, nl).trim();
          buffer = buffer.slice(nl + 1);
          if (!line) continue;
          try { dispatch(JSON.parse(line)); }
          catch (e) { globalThis.log.error(`prelude: parse error: ${e}`); }
        }
      });
    },
  };
}

const _transport = _createJsonLinesTransport();

// ─── Helpers ─────────────────────────────────────────────────────────────────

const _err = (e) => (e && e.message) ? e.message : String(e);

const _resolve = (fn, args, ok, fail) => {
  try {
    const r = fn(...args);
    if (r && typeof r.then === "function") r.then(ok, fail);
    else ok(r);
  } catch (e) { fail(e); }
};

// ─── Stateless globals (both v1 and v2) ──────────────────────────────────────

globalThis.log = {
  info: (message) => _transport.send({ type: "log", level: "info", message }),
  warn: (message) => _transport.send({ type: "log", level: "warn", message }),
  error: (message) => _transport.send({ type: "log", level: "error", message }),
};
globalThis.emit = (name, data) => _transport.send({ type: "event", name, data: data ?? {} });

// v1 legacy alias: pre-`serve()` apps reach for `globalThis.uploadFile`
// directly (e.g. Peppol prior to the v2 migration). v2 apps should use
// `ctx.uploadFile`. DO NOT remove without a migration plan.
globalThis.uploadFile = (content, filename, contentType) =>
  _uploadFile(content, filename, contentType);

console.log = (...a) => log.info(a.map(String).join(" "));
console.warn = (...a) => log.warn(a.map(String).join(" "));
console.error = (...a) => log.error(a.map(String).join(" "));
console.debug = console.log;

// ─── Internal state ──────────────────────────────────────────────────────────

let _ctx = null;
let _boot = null;
let _handlers = null;
let _started = false;

let _uploadSeq = 0;
const _pendingUploads = new Map();
let _opSeq = 0;
const _pendingOps = new Map();
let _sqlSeq = 0;
const _pendingSql = new Map();
let _saSeq = 0;
const _pendingSelf = new Map();

// ─── Primitives exposed via ctx ──────────────────────────────────────────────

function _uploadFile(content, filename, contentType) {
  if (!_ctx) return Promise.reject(new Error("uploadFile: worker not started yet"));
  const id = `upl_${++_uploadSeq}`;
  const data = typeof content === "string" ? new TextEncoder().encode(content) : content;
  const size = data.byteLength ?? data.length;
  log.info(`[storage] upload start: ${filename} (${size} bytes)`);
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      if (_pendingUploads.has(id)) {
        _pendingUploads.delete(id);
        reject(new Error("uploadFile: timeout waiting for upload URL"));
      }
    }, 30_000);
    _pendingUploads.set(id, { data, resolve, reject, timer });
    _transport.send({
      type: "storage_upload", id,
      name: filename || "upload",
      content_type: contentType || "application/octet-stream",
      size,
    });
  });
}

function _collectionOp(op, entity, data) {
  const id = `cop_${++_opSeq}`;
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      if (_pendingOps.has(id)) {
        _pendingOps.delete(id);
        reject(new Error(`collectionOp ${op} on ${entity}: timeout (30s)`));
      }
    }, 30_000);
    _pendingOps.set(id, { resolve, reject, timer });
    _transport.send({ type: "collection_op", id, op, entity, data });
  });
}

// SQL proxy: app SQL is executed by the core under the caller's RLS identity.
// The app never holds a DB connection. Returns { columns, rows, rowCount }.
// No identity travels on the message: the core binds it to this worker's sole
// in-flight unit of work, so the worker cannot name another user's identity.
function _sqlQuery(sql, params) {
  const id = `sql_${++_sqlSeq}`;
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      if (_pendingSql.has(id)) {
        _pendingSql.delete(id);
        reject(new Error("ctx.sql: timeout (30s)"));
      }
    }, 30_000);
    _pendingSql.set(id, { resolve, reject, timer });
    _transport.send({ type: "sql_query", id, sql, params: params ?? [] });
  });
}

// Privileged self-action over IPC (integrations). The core scopes the action
// to this worker's sole in-flight unit-of-work identity — no token to replay.
function _selfAction(action, params) {
  const id = `sa_${++_saSeq}`;
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      if (_pendingSelf.has(id)) {
        _pendingSelf.delete(id);
        reject(new Error(`selfAction ${action}: timeout (30s)`));
      }
    }, 30_000);
    _pendingSelf.set(id, { resolve, reject, timer });
    _transport.send({ type: "self_action", id, action, params: params ?? {} });
  });
}

// Per-call ctx. It carries NO identity token: the core resolves the caller's
// RLS identity from this worker's sole in-flight unit of work. During onStart
// (no active unit) ctx.sql denies (no identity) and ctx.collection runs
// BYPASSRLS on the self-schema.
function _makeCtx() {
  return {
    appId: _boot.app_id,
    runtimeUrl: _boot.runtime_url,
    credentials: _boot.credentials,
    agentConfig: _boot.agent_config,
    log: globalThis.log,
    emit: globalThis.emit,
    uploadFile: _uploadFile,
    sql: (sql, params = []) => _sqlQuery(sql, params),
    selfAction: (action, params = {}) => _selfAction(action, params),
    // Mediated integration call: the core resolves credentials via the
    // (app × user) binding and executes — the worker never sees a token.
    // `asUser` requires that user's own binding of this integration to this app.
    callIntegration: (integrationId, action, input = {}, asUser) =>
      _selfAction("call_integration", { integrationId, action, input, asUser }),
    collection(entity) {
      return {
        insert: (data) => _collectionOp("insert", entity, data),
        update: (data) => _collectionOp("update", entity, data),
        // Read ops use `where` as the equality map ({col: value}). Empty {} = full scan.
        find: (where = {}) => _collectionOp("find", entity, where),
        findOne: (where = {}) => _collectionOp("findOne", entity, where),
      };
    },
  };
}

// ─── Message dispatch ────────────────────────────────────────────────────────

function _dispatch(msg) {
  switch (msg.type) {
    case "storage_upload_url": {
      const p = _pendingUploads.get(msg.id);
      if (!p) return;
      _pendingUploads.delete(msg.id);
      clearTimeout(p.timer);
      fetch(msg.url, { method: "POST", body: p.data })
        .then(async (res) => {
          if (!res.ok) throw new Error(`upload failed: ${res.status} ${await res.text()}`);
          const { file_id } = await res.json();
          log.info(`[storage] upload done: ${file_id}`);
          p.resolve(file_id);
        })
        .catch((e) => { log.error(`storage upload error: ${e.message}`); p.reject(e); });
      return;
    }

    case "collection_op_result": {
      const p = _pendingOps.get(msg.id);
      if (!p) return;
      _pendingOps.delete(msg.id);
      clearTimeout(p.timer);
      msg.error ? p.reject(new Error(msg.error)) : p.resolve(msg.result);
      return;
    }

    case "sql_query_result": {
      const p = _pendingSql.get(msg.id);
      if (!p) return;
      _pendingSql.delete(msg.id);
      clearTimeout(p.timer);
      msg.error
        ? p.reject(new Error(msg.error))
        : p.resolve({ columns: msg.columns ?? [], rows: msg.rows ?? [], rowCount: msg.row_count ?? 0 });
      return;
    }

    case "self_action_result": {
      const p = _pendingSelf.get(msg.id);
      if (!p) return;
      _pendingSelf.delete(msg.id);
      clearTimeout(p.timer);
      msg.error ? p.reject(new Error(msg.error)) : p.resolve(msg.result);
      return;
    }

    case "discover": {
      _boot = msg;
      _ctx = _makeCtx();
      // v1 legacy: no serve() → worker responds to discover itself.
      if (!_handlers) return;
      _transport.send({
        type: "discover",
        protocol: PROTOCOL_VERSION,
        methods: Object.keys(_handlers.rpc ?? {}),
      });
      // onStart runs ONLY in the per-app lifecycle worker (run_onstart). Per-user
      // workers skip it: it seeds the self-schema under BYPASSRLS, which must not
      // run under a user identity, and the seeding already happened once.
      if (_handlers.onStart && !_started && msg.run_onstart) {
        _started = true;
        _resolve(_handlers.onStart, [_ctx],
          () => {},
          (e) => log.error(`onStart error: ${_err(e)}`),
        );
      }
      return;
    }

    case "rpc": {
      // v1 legacy: worker handles rpc itself.
      if (!_handlers) return;
      const fn = _handlers.rpc?.[msg.method];
      if (!fn) {
        _transport.send({ type: "rpc_response", id: msg.id, error: `unknown method: ${msg.method}` });
        return;
      }
      _resolve(fn, [msg.params, msg.caller ?? null, _makeCtx()],
        (r) => _transport.send({ type: "rpc_response", id: msg.id, result: r }),
        (e) => _transport.send({ type: "rpc_response", id: msg.id, error: _err(e) }),
      );
      return;
    }

    case "job": {
      // v1 legacy: worker handles jobs itself.
      if (!_handlers) return;
      const fn = _handlers.onJob;
      if (!fn) {
        _transport.send({ type: "job_result", id: msg.id, result: { ok: true } });
        return;
      }
      _resolve(fn, [msg.payload, msg.caller ?? null, _makeCtx()],
        (r) => _transport.send({ type: "job_result", id: msg.id, result: r ?? { ok: true } }),
        (e) => _transport.send({ type: "job_result", id: msg.id, error: _err(e) }),
      );
      return;
    }

    case "shutdown": {
      // v1 legacy: worker has its own shutdown handler.
      if (!_handlers) return;
      Promise.resolve(_handlers.onShutdown?.()).finally(() => process.exit(0));
      return;
    }
  }
}

// Wire transport → dispatch, then immediately pause stdin.
// Why: the prelude loads before the main script (--preload). For ESM modules
// with heavy imports (e.g. langchain), the event loop ticks during async module
// resolution. If stdin is flowing, the Discover message fires before the main
// script's readline handler is attached — v1 agents never see it and never boot.
// Pausing here keeps data buffered in the pipe until a consumer resumes:
//   - v2 apps: serve() calls resume()
//   - v1 apps: adding a "data" listener (readline or raw) triggers resume via newListener
_transport.onMessage(_dispatch);
process.stdin.pause();
process.stdin.on("newListener", (ev) => { if (ev === "data") process.stdin.resume(); });

// ─── Public API ──────────────────────────────────────────────────────────────

globalThis.serve = (handlers) => {
  if (_handlers) throw new Error("serve() called twice");
  if (!handlers) { _handlers = {}; process.stdin.resume(); return; }
  // Detect old signature: serve({ methodName: fn, ... })
  // vs new signature:     serve({ rpc, onStart, onJob, onShutdown })
  // If any reserved key is present, it's the new signature.
  const isNew = ["rpc", "onStart", "onJob", "onShutdown"].some((k) => k in handlers);
  if (!isNew) {
    globalThis.log.warn("serve() called with flat signature (deprecated). Use serve({ rpc: { ... } }) instead.");
  }
  _handlers = isNew ? handlers : { rpc: handlers };
  process.stdin.resume();
};
