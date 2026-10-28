import { createInterface } from "node:readline";
import { readFile, readdir, realpath } from "node:fs/promises";
import { resolve, sep } from "node:path";
import { ChatAnthropic } from "@langchain/anthropic";
import { ChatOpenAI } from "@langchain/openai";
import { ChatBedrockConverse } from "@langchain/aws";
import { publicErrorCode } from "./errors";
import { runHarness, type Boot, type Invoke } from "./harness";

const write = (m: unknown) => process.stdout.write(JSON.stringify(m) + "\n");
const pending = new Map<string, { resolve: (value: any) => void; reject: (error: Error) => void; timer: ReturnType<typeof setTimeout> }>();
let boot: Boot | undefined;
function request(message: Record<string, unknown>, timeout: number): Promise<any> {
  const id = crypto.randomUUID();
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => { pending.delete(id); reject(new Error("Core request timed out; outcome unknown")); }, timeout);
    pending.set(id, { resolve, reject, timer });
    write({ ...message, id, call_id: id });
  });
}
async function readAppFile(path: string): Promise<string> {
  const root = await realpath(process.cwd());
  const file = await realpath(resolve(root, path));
  if (!file.startsWith(root + sep)) throw new Error("Agent instruction path escapes the app");
  return readFile(file, "utf8");
}
async function assets() {
  const result: Record<string, string> = {};
  try { result["/instructions/AGENTS.md"] = await readAppFile("AGENTS.md"); } catch (e: any) { if (e.code !== "ENOENT") throw e; }
  try {
    for (const item of await readdir("agent/skills", { recursive: true, withFileTypes: true })) {
      if (item.isFile()) {
        const path = resolve(item.parentPath, item.name);
        result["/skills/" + path.slice(resolve("agent/skills").length + 1).split(sep).join("/")] = await readAppFile(path);
      }
    }
  } catch (e: any) { if (e.code !== "ENOENT") throw e; }
  return result;
}
function createModel(llm: NonNullable<Invoke["llm"]>): ChatAnthropic | ChatOpenAI | ChatBedrockConverse {
  const options = { model: llm.model, maxRetries: 0 };
  switch (llm.provider) {
    case "rootcx":
      return new ChatAnthropic({ ...options, apiKey: process.env.ROOTCX_API_KEY, anthropicApiUrl: process.env.ROOTCX_LLM_ENDPOINT });
    case "anthropic":
      return new ChatAnthropic(options);
    case "openai":
      return new ChatOpenAI({ ...options, ...(process.env.OPENAI_BASE_URL ? { streamUsage: false, configuration: { baseURL: process.env.OPENAI_BASE_URL } } : {}) });
    case "bedrock":
    case "bedrock_converse":
      return new ChatBedrockConverse({ ...options, region: process.env.AWS_REGION || "us-east-1" });
    default:
      throw new Error(`Unsupported model provider: ${llm.provider}`);
  }
}
async function invoke(m: Invoke) {
  try {
    if (!boot || !m.llm) throw new Error("Managed agent configuration or model is missing");
    let prompt = "";
    try { prompt = await readAppFile(boot.definition.systemPrompt ?? "agent/system.md"); }
    catch (e: any) { if (e.code !== "ENOENT") throw e; }
    const model = createModel(m.llm);
    const result = await runHarness(boot, m, model, {
      state: (op, namespace, key, value) => request({ type: "agent_state", invoke_id: m.invoke_id, op, namespace, key, value }, 60_000),
      callTool: (tool_name, args) => request({ type: "agent_tool_call", invoke_id: m.invoke_id, tool_name, args }, 15 * 60_000),
      chunk: delta => write({ type: "agent_chunk", invoke_id: m.invoke_id, delta }),
    }, prompt, await assets());
    write({ type: "agent_done", invoke_id: m.invoke_id, ...result });
  } catch (error: any) {
    const message = String(error.message ?? error);
    console.error(`[managed-harness] ${message.slice(0, 500)}`);
    write({ type: "agent_error", invoke_id: m.invoke_id, error: publicErrorCode(error) });
  }
}
createInterface({ input: process.stdin }).on("line", line => {
  let m: any; try { m = JSON.parse(line); } catch { return; }
  if (m.type === "discover") {
    boot = m.agent_config;
    for (const [k, v] of Object.entries(m.credentials ?? {})) process.env[k] = String(v);
  } else if (["agent_tool_result", "agent_state_result"].includes(m.type)) {
    const id = m.call_id ?? m.id, call = pending.get(id);
    if (call) { clearTimeout(call.timer); pending.delete(id); m.error ? call.reject(new Error(m.error)) : call.resolve(m.result); }
  } else if (m.type === "agent_invoke") void invoke(m);
  else if (m.type === "rpc") write({ type: "rpc_response", id: m.id, error: "Managed harness exposes agent invocation only" });
  else if (m.type === "job") write({ type: "job_result", id: m.id, error: "Agent jobs must enter through governed agent invocation" });
});
write({ type: "discover", protocol: 1 });
