interface Caller { userId: string; username: string; authToken?: string }
type Handler = (params: any, caller: Caller | null) => any;

const send = (msg: Record<string, unknown>) =>
  process.stdout.write(JSON.stringify(msg) + "\n");

const handlers: Record<string, Handler> = {
  ping: () => ({ pong: true }),
  echo: (params) => params,
  whoami: (_, caller) => caller ?? { error: "not authenticated" },
};

let buffer = "";
process.stdin.setEncoding("utf-8");
process.stdin.on("data", (chunk: string) => {
  buffer += chunk;
  let nl: number;
  while ((nl = buffer.indexOf("\n")) !== -1) {
    const line = buffer.slice(0, nl).trim();
    buffer = buffer.slice(nl + 1);
    if (!line) continue;

    try {
      const msg = JSON.parse(line);
      switch (msg.type) {
        case "discover":
          send({ type: "discover", methods: Object.keys(handlers) });
          break;
        case "rpc": {
          const fn = handlers[msg.method];
          if (fn) send({ type: "rpc_response", id: msg.id, result: fn(msg.params, msg.caller) });
          else send({ type: "rpc_response", id: msg.id, error: `unknown method: ${msg.method}` });
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
