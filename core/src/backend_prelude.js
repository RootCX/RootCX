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
//       ctx.appId, ctx.runtimeUrl, ctx.databaseUrl,
//       ctx.credentials, ctx.agentConfig,
//       ctx.log, ctx.emit,
//       ctx.uploadFile(content, filename, contentType),
//       ctx.collection(entity).insert(data),
//       ctx.collection(entity).update(data),
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
let _handlers = null;
let _started = false;

let _uploadSeq = 0;
const _pendingUploads = new Map();
let _opSeq = 0;
const _pendingOps = new Map();

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

function _makeCtx(msg) {
  return {
    appId: msg.app_id,
    runtimeUrl: msg.runtime_url,
    databaseUrl: msg.database_url,
    credentials: msg.credentials,
    agentConfig: msg.agent_config,
    log: globalThis.log,
    emit: globalThis.emit,
    uploadFile: _uploadFile,
    collection(entity) {
      return {
        insert: (data) => _collectionOp("insert", entity, data),
        update: (data) => _collectionOp("update", entity, data),
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

    case "discover": {
      _ctx = _makeCtx(msg);
      // v1 legacy: no serve() → worker responds to discover itself.
      if (!_handlers) return;
      _transport.send({
        type: "discover",
        protocol: PROTOCOL_VERSION,
        methods: Object.keys(_handlers.rpc ?? {}),
      });
      if (_handlers.onStart && !_started) {
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
      _resolve(fn, [msg.params, msg.caller ?? null, _ctx],
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
      _resolve(fn, [msg.payload, msg.caller ?? null, _ctx],
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

// Wire transport → dispatch
_transport.onMessage(_dispatch);

// ─── Public API ──────────────────────────────────────────────────────────────

globalThis.serve = (handlers) => {
  if (_handlers) throw new Error("serve() called twice");
  if (!handlers) { _handlers = {}; return; }
  // Detect old signature: serve({ methodName: fn, ... })
  // vs new signature:     serve({ rpc, onStart, onJob, onShutdown })
  // If any reserved key is present, it's the new signature.
  const isNew = ["rpc", "onStart", "onJob", "onShutdown"].some((k) => k in handlers);
  if (!isNew) {
    globalThis.log.warn("serve() called with flat signature (deprecated). Use serve({ rpc: { ... } }) instead.");
  }
  _handlers = isNew ? handlers : { rpc: handlers };
};
