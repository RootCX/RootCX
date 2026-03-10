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

globalThis.serve = (handlers, opts) => {
  const methods = Object.keys(handlers);
  let buffer = "";

  process.stdin.setEncoding("utf-8");
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
