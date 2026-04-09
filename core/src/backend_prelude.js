const _write = (msg) => process.stdout.write(JSON.stringify(msg) + "\n");
const _err = (e) => e?.message ?? String(e);

const _resolve = (fn, args, ok, fail) => {
  try {
    const r = fn(...args);
    if (r && typeof r.then === "function") r.then(ok, fail);
    else ok(r);
  } catch (e) { fail(e); }
};

globalThis.log = {
  info: (message) => _write({ type: "log", level: "info", message }),
  warn: (message) => _write({ type: "log", level: "warn", message }),
  error: (message) => _write({ type: "log", level: "error", message }),
};

globalThis.emit = (name, data) => _write({ type: "event", name, data: data ?? {} });

// Redirect console to IPC log channel — raw stdout would corrupt the JSON-lines protocol
console.log = (...a) => log.info(a.map(String).join(" "));
console.warn = (...a) => log.warn(a.map(String).join(" "));
console.error = (...a) => log.error(a.map(String).join(" "));
console.debug = console.log;

let _runtimeUrl = process.env.ROOTCX_RUNTIME_URL || "";
let _uploadSeq = 0;
const _pendingUploads = new Map();

globalThis.uploadFile = (content, filename, contentType) => {
  if (!_runtimeUrl) throw new Error("uploadFile: runtime not started yet");
  log.info(`[storage] upload start: ${filename} (${content?.length ?? 0} bytes)`);
  const id = `upl_${++_uploadSeq}`;
  const data = typeof content === "string" ? new TextEncoder().encode(content) : content;
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      if (_pendingUploads.has(id)) {
        _pendingUploads.delete(id);
        reject(new Error("uploadFile: timeout waiting for upload URL"));
      }
    }, 30_000);
    _pendingUploads.set(id, { data, resolve, reject, timer });
    _write({
      type: "storage_upload", id,
      name: filename || "upload",
      content_type: contentType || "application/octet-stream",
      size: data.byteLength || data.length,
    });
  });
};

// Global IPC listener for prelude-managed messages (storage_upload_url).
// Runs alongside any worker-specific stdin listener (both fire per message in Bun/Node).
process.stdin.setEncoding("utf-8");
let _preludeBuffer = "";
process.stdin.on("data", (chunk) => {
  _preludeBuffer += chunk;
  let nl;
  while ((nl = _preludeBuffer.indexOf("\n")) !== -1) {
    const line = _preludeBuffer.slice(0, nl).trim();
    _preludeBuffer = _preludeBuffer.slice(nl + 1);
    if (!line) continue;
    try {
      const msg = JSON.parse(line);
      if (msg.type === "storage_upload_url") {
        const pending = _pendingUploads.get(msg.id);
        if (pending) {
          _pendingUploads.delete(msg.id);
          clearTimeout(pending.timer);
          fetch(msg.url, { method: "POST", body: pending.data })
            .then(async (res) => {
              if (!res.ok) throw new Error(`upload failed: ${res.status} ${await res.text()}`);
              const { file_id } = await res.json();
              log.info(`[storage] upload done: ${file_id}`);
              pending.resolve(file_id);
            })
            .catch((e) => { log.error(`storage upload error: ${e.message}`); pending.reject(e); });
        }
      }
      if (msg.type === "discover") {
        _runtimeUrl = msg.runtime_url || _runtimeUrl;
      }
    } catch {}
  }
});

globalThis.serve = (handlers, opts) => {
  const methods = Object.keys(handlers);
  let buffer = "";

  process.stdin.on("data", (chunk) => {
    buffer += chunk;
    let nl;
    while ((nl = buffer.indexOf("\n")) !== -1) {
      const line = buffer.slice(0, nl).trim();
      buffer = buffer.slice(nl + 1);
      if (!line) continue;

      try {
        const msg = JSON.parse(line);
        switch (msg.type) {
          case "discover":
            _write({ type: "discover", methods });
            opts?.onStart?.({
              appId: msg.app_id,
              runtimeUrl: msg.runtime_url,
              databaseUrl: msg.database_url,
              credentials: msg.credentials,
            });
            break;
          case "rpc": {
            const h = handlers[msg.method];
            if (!h) { _write({ type: "rpc_response", id: msg.id, error: `unknown method: ${msg.method}` }); break; }
            _resolve(h, [msg.params, msg.caller ?? null],
              (r) => _write({ type: "rpc_response", id: msg.id, result: r }),
              (e) => { _write({ type: "rpc_response", id: msg.id, error: _err(e) }); opts?.onError?.(e, msg.method); },
            );
            break;
          }
          case "job":
            _resolve(opts?.onJob ?? (() => ({ ok: true })), [msg.payload],
              (r) => _write({ type: "job_result", id: msg.id, result: r ?? { ok: true } }),
              (e) => { _write({ type: "job_result", id: msg.id, error: _err(e) }); opts?.onError?.(e, "job"); },
            );
            break;
          case "shutdown":
            Promise.resolve(opts?.onShutdown?.()).finally(() => process.exit(0));
        }
      } catch (e) {
        _write({ type: "log", level: "error", message: `parse error: ${e}` });
      }
    }
  });
};
