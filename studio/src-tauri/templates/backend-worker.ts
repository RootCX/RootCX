/**
 * RootCX Backend Worker — JSON-line IPC over stdin/stdout.
 *
 * Every RPC message includes a `caller` field:
 *   - Authenticated: { userId: "...", username: "alice" }
 *   - Anonymous:     null
 */

interface Caller { userId: string; username: string }
type Handler = (params: any, caller: Caller | null) => any;

const send = (msg: Record<string, unknown>) => {
  process.stdout.write(JSON.stringify(msg) + "\n");
};

const handlers: Record<string, Handler> = {
  ping: () => ({ pong: true }),
  echo: (params) => params,
  whoami: (_params, caller) => {
    if (!caller) return { error: "not authenticated" };
    return { userId: caller.userId, username: caller.username };
  },
};

process.stdin.setEncoding("utf-8");

let buffer = "";
process.stdin.on("data", (chunk: string) => {
  buffer += chunk;
  let newline: number;
  while ((newline = buffer.indexOf("\n")) !== -1) {
    const line = buffer.slice(0, newline).trim();
    buffer = buffer.slice(newline + 1);
    if (!line) continue;

    try {
      const msg = JSON.parse(line);

      switch (msg.type) {
        case "discover":
          send({ type: "discover_result", methods: Object.keys(handlers) });
          break;

        case "rpc": {
          const fn = handlers[msg.method];
          if (fn) {
            send({ type: "rpc_response", id: msg.id, result: fn(msg.params, msg.caller) });
          } else {
            send({ type: "rpc_response", id: msg.id, error: `unknown method: ${msg.method}` });
          }
          break;
        }

        case "job":
          send({ type: "job_result", id: msg.id, result: { ok: true } });
          break;

        case "shutdown":
          process.exit(0);
      }
    } catch (e) {
      send({ type: "log", level: "error", message: `parse error: ${e}` });
    }
  }
});
