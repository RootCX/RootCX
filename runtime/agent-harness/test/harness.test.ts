import { test, expect } from "bun:test";
import { BaseChatModel } from "@langchain/core/language_models/chat_models";
import { AIMessage, isAIMessage, isToolMessage, type BaseMessage } from "@langchain/core/messages";
import { runHarness, type Boot } from "../src/harness";
import { CoreState, type StateRequest } from "../src/state";

class ScriptModel extends BaseChatModel {
  seen: BaseMessage[][] = [];
  constructor(readonly steps: ((messages: BaseMessage[]) => AIMessage)[]) { super({}); }
  _llmType() { return "scripted-boundary-test"; }
  bindTools() { return this; }
  async _generate(messages: BaseMessage[]) {
    this.seen.push(messages);
    const step = this.steps.shift();
    if (!step) throw new Error("Unexpected additional model call");
    const message = step(messages);
    return { generations: [{ text: message.text, message }] };
  }
}
function database() {
  const rows = new Map<string, any>();
  const request: StateRequest = async (op, ns, key, value) => {
    const id = `${ns}:${key}`;
    if (op === "get") return rows.get(id) ?? null;
    if (op === "put") { rows.set(id, structuredClone(value)); return true; }
    if (op === "delete") return rows.delete(id);
    return [...rows.keys()].filter(k => k.startsWith(`${ns}:${key}`)).map(k => k.slice(ns.length + 1)).sort().filter(k => k > (value ?? "")).slice(0, 500);
  };
  return { rows, request };
}
const aiTool = (name: string, args: object) => new AIMessage({ content: "", tool_calls: [{ id: crypto.randomUUID(), name, args, type: "tool_call" }] });
const boot: Boot = { max_turns: 12, definition: { name: "Boundary test", memory: { enabled: true } }, tool_descriptors: [
  { name: "call_action", description: "Execute a business action", inputSchema: { type: "object", properties: { input: { type: "object" } }, required: ["input"] } },
] };

test("a clarification resumes saved tools and notes after rebuilding the entire harness", async () => {
  const db = database(), writes: any[] = [];
  const bridge = { state: db.request, chunk() {}, callTool: async (name: string, args: any) => { writes.push({ name, args }); return { saved: true }; } };
  const first = new ScriptModel([
    () => aiTool("write_file", { file_path: "/workspace/import.json", content: '{"smallTTC":13.5,"largeTTC":16.5,"status":"awaiting VAT"}' }),
    () => new AIMessage("Quel taux de TVA ?"),
  ]);
  await runHarness(boot, { invoke_id: "one", session_id: "session", message: "Configure from my quote" }, first, bridge, "");
  const second = new ScriptModel([
    messages => {
      expect(JSON.stringify(messages)).toContain("smallTTC");
      expect(JSON.stringify(messages)).toContain("Quel taux de TVA");
      return aiTool("read_file", { file_path: "/workspace/import.json" });
    },
    messages => {
      expect(messages.at(-1)!.text).toContain("13.5");
      return aiTool("call_action", { input: { priceTTC: 13.5, vat: 6 } });
    },
    () => new AIMessage("Prestation enregistrée."),
  ]);
  const result = await runHarness(boot, { invoke_id: "two", session_id: "session", message: "6 %" }, second, bridge, "");
  expect(result.response).toBe("Prestation enregistrée.");
  expect(writes).toEqual([{ name: "call_action", args: { input: { priceTTC: 13.5, vat: 6 } } }]);
});

test("binary state exceeds one IPC frame without truncation or partial publication", async () => {
  const db = database(), state = new CoreState(db.request, "files");
  const bytes = crypto.getRandomValues(new Uint8Array(65_536));
  const content = new Uint8Array(1_500_000).map((_, i) => bytes[i % bytes.length]);
  await state.put("document", { content });
  expect((await new CoreState(db.request, "files").get<{content: Uint8Array}>("document"))!.content).toEqual(content);
  expect(Math.max(...[...db.rows.values()].map(v => JSON.stringify(v).length))).toBeLessThan(600_000);
  const broken: StateRequest = async (...args) => {
    if (args[0] === "put" && args[2].endsWith("/1")) throw new Error("interrupted write");
    return db.request(...args);
  };
  content[0]++;
  await expect(new CoreState(broken, "files").put("document", { content })).rejects.toThrow("interrupted write");
  expect((await state.get<{content: Uint8Array}>("document"))!.content[0]).toBe(bytes[0]);
});

test("model-call budget stops a continuing tool loop", async () => {
  const db = database();
  const model = new ScriptModel(Array.from({ length: 10 }, () => () => aiTool("ls", { path: "/" })));
  await expect(runHarness({ ...boot, max_turns: 2 }, { invoke_id: "loop", session_id: "loop", message: "continue" }, model,
    { state: db.request, chunk() {}, callTool: async () => { throw new Error("unexpected business tool"); } }, "")).rejects.toThrow("model-call limit");
  expect(model.seen).toHaveLength(2);
});

test("an internal subagent uses the same governed bridge and preserves a denial", async () => {
  const db = database(), calls: unknown[] = [];
  const model = new ScriptModel([
    () => aiTool("task", { subagent_type: "general-purpose", description: "Attempt the requested business operation" }),
    () => aiTool("call_action", { input: { operation: "restricted" } }),
    messages => {
      expect(messages.at(-1)!.text).toContain("permission denied");
      return new AIMessage("The operation was denied; nothing was saved.");
    },
    messages => {
      expect(messages.at(-1)!.text).toContain("nothing was saved");
      return new AIMessage("Permission required.");
    },
  ]);
  const result = await runHarness(boot, { invoke_id: "delegate", session_id: "delegate", message: "Delegate this operation" }, model,
    { state: db.request, chunk() {}, callTool: async (name, args) => { calls.push({ name, args }); throw new Error("permission denied"); } }, "");
  expect(result.response).toBe("Permission required.");
  expect(calls).toEqual([{ name: "call_action", args: { input: { operation: "restricted" } } }]);
});

test("delegation cannot multiply the invocation model-call budget", async () => {
  const db = database();
  const model = new ScriptModel([
    () => aiTool("task", { subagent_type: "general-purpose", description: "Analyze" }),
    () => aiTool("ls", { path: "/" }),
    () => new AIMessage("This third model call must never execute"),
  ]);
  await expect(runHarness({ ...boot, max_turns: 2 }, { invoke_id: "budget", session_id: "budget", message: "Delegate" }, model,
    { state: db.request, chunk() {}, callTool: async () => { throw new Error("unexpected business tool"); } }, "")).rejects.toThrow("model-call limit");
  expect(model.seen).toHaveLength(2);
});

test("reported usage includes subagents but never re-counts saved history", async () => {
  const db = database();
  const bridge = { state: db.request, chunk() {}, callTool: async () => { throw new Error("unexpected business tool"); } };
  const usage = (message: AIMessage, total: number) => {
    message.usage_metadata = { input_tokens: total - 1, output_tokens: 1, total_tokens: total };
    return message;
  };
  const first = await runHarness(boot, { invoke_id: "usage-one", session_id: "usage", message: "Hello" },
    new ScriptModel([() => usage(new AIMessage("Hello"), 100)]), bridge, "");
  expect(first.tokens).toBe(100);
  const second = await runHarness(boot, { invoke_id: "usage-two", session_id: "usage", message: "Delegate an analysis" },
    new ScriptModel([
      () => usage(aiTool("task", { subagent_type: "general-purpose", description: "Analyze" }), 10),
      () => usage(new AIMessage("Analysis complete"), 20),
      () => usage(new AIMessage("Done"), 30),
      ]), bridge, "");
  expect(second.tokens).toBe(60);
});

test("disabling memory does not persist workspace or checkpoints through Core", async () => {
  const model = new ScriptModel([
    () => aiTool("write_file", { file_path: "/workspace/private.txt", content: "ephemeral" }),
    () => new AIMessage("Done"),
  ]);
  const result = await runHarness({ ...boot, definition: { ...boot.definition, memory: { enabled: false } } },
    { invoke_id: "ephemeral", session_id: "ephemeral", message: "Take a temporary note" }, model,
    { state: async () => { throw new Error("unexpected persistence"); }, chunk() {}, callTool: async () => { throw new Error("unexpected business tool"); } }, "");
  expect(result.response).toBe("Done");
});

import { publicErrorCode } from "../src/errors";
test("provider errors retain channel codes without exposing provider details", () => {
  for (const [error, expected] of [
    [{ cause: { status: 402, error: { error: { type: "credits_exhausted" } } } }, "AI_CREDITS_EXHAUSTED"],
    [{ status: 402, error: { type: "credits_reserved" } }, "AI_CREDITS_RESERVED"],
    [{ status: 429 }, "AI_RATE_LIMITED"], [{ status: 503 }, "AI_UNAVAILABLE"],
    [new Error("private provider payload"), "AGENT_FAILED"],
  ] as const) expect(publicErrorCode(error)).toBe(expected);
});

test("a provider refusal keeps its channel code after crossing the agent middleware", async () => {
  const refusal = Object.assign(new Error("402 Insufficient credits"), { status: 402, error: { error: { type: "credits_exhausted" } } });
  const model = new ScriptModel([() => { throw refusal; }]);
  const failure = await runHarness(boot, { invoke_id: "credits", session_id: "credits", message: "Create the quote" }, model,
    { state: database().request, chunk() {}, callTool: async () => ({}) }, "").catch(error => error);
  expect(publicErrorCode(failure)).toBe("AI_CREDITS_EXHAUSTED");
});

test("business actions issued together finish in order even when the first one fails", async () => {
  const db=database(), order:string[]=[];
  const model=new ScriptModel([
    ()=>new AIMessage({content:"",tool_calls:["first","second"].map(operation=>({id:operation,name:"call_action",args:{input:{operation}},type:"tool_call" as const}))}),
    ()=>new AIMessage("The first action failed."),
  ]);
  await runHarness(boot,{invoke_id:"ordered",session_id:"ordered",message:"Run both"},model,
    {state:db.request,chunk(){},callTool:async(_name,args:any)=>{
      const operation=args.input.operation;order.push(operation+" start");
      await new Promise(resolve=>setTimeout(resolve,10));order.push(operation+" end");
      if(operation==="first")throw Error("failure");return {saved:true};
    }},"");
  expect(order).toEqual(["first start","first end","second start","second end"]);
});

test("a completed action and final answer succeed at the model budget without a second generation", async () => {
  for (const memory of [true, false]) {
    const db = database(), chunks: string[] = [], writes: unknown[] = [];
    const model = new ScriptModel([
      () => aiTool("call_action", { input: { save: true } }),
      () => new AIMessage("Recorded. The requested result is ready. Nothing else was changed."),
    ]);
    const result = await runHarness({ ...boot, max_turns: 2, definition: { ...boot.definition, memory: { enabled: memory } } },
      { invoke_id: "budget", session_id: "budget", message: "Save the record" }, model,
      { state: db.request, chunk: c => chunks.push(c), callTool: async (_name, args) => { writes.push(args); return { saved: true }; } }, "");
    expect(writes, `memory=${memory}`).toHaveLength(1);
    expect(model.seen).toHaveLength(2);
    expect(chunks).toEqual([result.response]);
    if (memory) {
      const next = new ScriptModel([messages => {
        expect(JSON.stringify(messages)).toContain(result.response);
        return new AIMessage("Acknowledged.");
      }]);
      await runHarness(boot, { invoke_id: "next", session_id: "budget", message: "Continue" }, next,
        { state: db.request, chunk() {}, callTool: async () => { throw Error("Unexpected replay"); } }, "");
    }
  }
});

test("deployed instructions refresh for parent and isolated subagent and stay read-only", async () => {
  const db = database();
  const files = new CoreState(db.request, "files");
  // An old runtime persisted its bundled skill into this conversation.
  const { CoreStore } = await import("../src/store");
  const { StoreBackend } = await import("deepagents");
  const legacy = new StoreBackend({ store: new CoreStore(files), fileFormat: "v2" });
  await legacy.uploadFiles([["/skills/obsolete/SKILL.md", Buffer.from("---\nname: obsolete\ndescription: OLD_RELEASE_INSTRUCTION\n---\nOld task guidance")]]);
  const personal = new StoreBackend({ store: new CoreStore(new CoreState(db.request, "memory")), fileFormat: "v2" });
  await personal.uploadFiles([["/AGENTS.md", Buffer.from("PERSONAL_PREFERENCE")]]);
  const bridge = {state:db.request, chunk() {}, callTool:async()=>{throw Error("Unexpected business action");}};
  for (const revision of ["FIRST_APP_RELEASE", "SECOND_APP_RELEASE"]) {
    const model = new ScriptModel([
      messages => {
        const system = messages.filter(m=>m._getType()==="system").map(m=>m.text).join("\n");
        expect(system, revision).toContain(revision);
        expect(system, revision).toContain("PERSONAL_PREFERENCE");
        expect(system, revision).not.toContain("OLD_RELEASE_INSTRUCTION");
        if (revision === "SECOND_APP_RELEASE") expect(system).not.toContain("FIRST_APP_RELEASE");
        return aiTool("task", { subagent_type: "general-purpose", description: "Check the deployed instructions" });
      },
      messages => {
        const system = messages.filter(m=>m._getType()==="system").map(m=>m.text).join("\n");
        expect(system, revision).toContain(revision);
        expect(system, revision).not.toContain("PERSONAL_PREFERENCE");
        expect(system, revision).not.toContain("OLD_RELEASE_INSTRUCTION");
        if (revision === "SECOND_APP_RELEASE") expect(system).not.toContain("FIRST_APP_RELEASE");
        return aiTool("edit_file", { file_path: "/instructions/AGENTS.md", old_string: revision, new_string: "REWRITTEN" });
      },
      messages => {
        expect(messages.at(-1)!.text.toLowerCase()).toContain("denied");
        return new AIMessage("Read-only.");
      },
      () => new AIMessage("Ready."),
    ]);
    await runHarness(boot, {invoke_id:revision,session_id:"assets",message:"Continue"}, model, bridge, "", {
      "/instructions/AGENTS.md": revision,
    });
  }
});

test("a new turn after losing a tool result does not replay the uncertain write", async () => {
  const db = database();
  let disconnected = false, writes = 0;
  const state: StateRequest = (...args) => {
    if (disconnected) throw new Error("Core connection lost");
    return db.request(...args);
  };
  await expect(runHarness(boot, { invoke_id: "lost", session_id: "resume", message: "Record once" },
    new ScriptModel([() => aiTool("call_action", { input: { name: "once" } })]),
    { state, chunk() {}, callTool: async () => { writes++; disconnected = true; throw new Error("Core connection lost"); } }, ""))
    .rejects.toThrow();
  const resumed = new ScriptModel([messages => {
    // The model must be able to reason about uncertainty, with valid tool-call history.
    const pending = new Set<string>();
    for (const message of messages) {
      if (isAIMessage(message)) for (const call of (message as AIMessage).tool_calls ?? []) pending.add(call.id!);
      if (isToolMessage(message)) pending.delete((message as any).tool_call_id);
    }
    expect(pending.size).toBe(0);
    expect(messages.filter(isToolMessage).map(m => m.text).join(" ")).toContain("outcome is unknown");
    return new AIMessage("Check the recorded outcome before continuing.");
  }]);
  await runHarness(boot, { invoke_id: "resume", session_id: "resume", message: "Continue" }, resumed,
    { state: db.request, chunk() {}, callTool: async () => { writes++; return {}; } }, "");
  expect(writes).toBe(1);
});


test("configured skills reach the first model call and isolated subagents without model-selected reads", async () => {
  const db = database();
  const config = { ...boot, definition: { ...boot.definition, pinnedSkills: ["quote"] } };
  const assets = { "/skills/quote/SKILL.md": "---\nname: quote\ndescription: Quote tasks only\n---\nQUOTE_BEHAVIOR_V1" };
  function assertPinned(messages: BaseMessage[], body: string) {
    const pins = messages.filter(m => m.additional_kwargs.lc_source === "pinned_skill");
    expect(pins.at(-1)?.text).toContain(body);
    expect(pins.at(-1)?.additional_kwargs.skill).toMatchObject({ name: "quote" });
  }
  const bridge = { state: db.request, chunk() {}, callTool: async () => { throw new Error("Unexpected business call"); } };
  const model = new ScriptModel([
    messages => { assertPinned(messages, "QUOTE_BEHAVIOR_V1"); return aiTool("task", { subagent_type: "general-purpose", description: "Inspect this quote" }); },
    messages => { assertPinned(messages, "QUOTE_BEHAVIOR_V1"); return new AIMessage("Inspected"); },
    () => new AIMessage("Ready"),
  ]);
  await runHarness(config, { invoke_id: "pinned", session_id: "pins", message: "Quote please" }, model, bridge, "", assets);
  const next = new ScriptModel([messages => {
    assertPinned(messages, "QUOTE_BEHAVIOR_V2");
    return new AIMessage("Updated");
  }]);
  await runHarness(config, { invoke_id: "repinned", session_id: "pins", message: "Change quantities" }, next, bridge, "",
    { "/skills/quote/SKILL.md": assets["/skills/quote/SKILL.md"].replace("V1", "V2") });
});

test("a missing configured skill fails before calling the model", async () => {
  const model = new ScriptModel([]);
  await expect(runHarness({ ...boot, definition: { ...boot.definition, pinnedSkills: ["missing"] } },
    { invoke_id: "missing", session_id: "missing", message: "Quote" }, model,
    { state: database().request, chunk() {}, callTool: async () => null }, "")).rejects.toThrow("Pinned skill unavailable");
  expect(model.seen).toHaveLength(0);
});
