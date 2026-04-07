import { createInterface } from "readline";
import postgres from "postgres";

interface Caller { userId: string; username: string; authToken?: string }

const write = (m: any) => process.stdout.write(JSON.stringify(m) + "\n");
const rl = createInterface({ input: process.stdin });
let sql: ReturnType<typeof postgres>;
let runtimeUrl: string;
let appId: string;

rl.on("line", (l) => {
  let m: any;
  try { m = JSON.parse(l); } catch { return; }

  switch (m.type) {
    case "discover":
      appId = m.app_id;
      runtimeUrl = m.runtime_url;
      sql = postgres(m.database_url);
      write({ type: "discover", methods: ["ping"] });
      break;
    case "rpc":
      handleRpc(m);
      break;
    case "job":
      handleJob(m);
      break;
    case "shutdown":
      process.exit(0);
  }
});

async function handleRpc(m: any) {
  try {
    const result = await dispatch(m.method, m.params ?? {}, m.caller);
    write({ type: "rpc_response", id: m.id, result });
  } catch (e: any) {
    write({ type: "rpc_response", id: m.id, error: e.message });
  }
}

async function handleJob(m: any) {
  try {
    const result = await runJob(m.payload, m.caller);
    write({ type: "job_result", id: m.id, result });
  } catch (e: any) {
    write({ type: "job_result", id: m.id, error: e.message });
  }
}

async function dispatch(method: string, params: any, caller: Caller | null): Promise<any> {
  switch (method) {
    case "ping": return { pong: true };
    default: throw new Error(`unknown method: ${method}`);
  }
}

async function runJob(payload: any, caller: Caller | null): Promise<any> {
  return { done: true };
}
