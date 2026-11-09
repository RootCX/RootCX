import { test, expect } from "bun:test";
import { BaseChatModel } from "@langchain/core/language_models/chat_models";
import { AIMessage, isHumanMessage, isToolMessage, type BaseMessage } from "@langchain/core/messages";
import { runHarness, type Boot, type Bridge, type InboxMessage } from "../src/harness";
import type { StateRequest } from "../src/state";

class ScriptModel extends BaseChatModel {
  seen: BaseMessage[][] = [];
  constructor(readonly steps: ((messages: BaseMessage[]) => AIMessage)[]) { super({}); }
  _llmType() { return "scripted-steering-test"; }
  bindTools() { return this; }
  async _generate(messages: BaseMessage[]) {
    this.seen.push(messages);
    const step = this.steps.shift();
    if (!step) throw new Error("Unexpected additional model call");
    const message = step(messages);
    return { generations: [{ text: message.text, message }] };
  }
}
function database(): StateRequest {
  const rows = new Map<string, any>();
  return async (op, ns, key, value) => {
    const id = `${ns}:${key}`;
    if (op === "get") return rows.get(id) ?? null;
    if (op === "put") { rows.set(id, structuredClone(value)); return true; }
    if (op === "delete") return rows.delete(id);
    return [...rows.keys()].filter(k => k.startsWith(`${ns}:${key}`)).map(k => k.slice(ns.length + 1)).sort().filter(k => k > (value ?? "")).slice(0, 500);
  };
}
/** Core's run inbox: messages wait until drained; a final drain with nothing waiting closes it. */
function mailbox() {
  const waiting: InboxMessage[] = [];
  let stop = false, closed = false;
  const drains: boolean[] = [];
  return {
    drains,
    get closed() { return closed; },
    send(message: InboxMessage) { if (closed) throw new Error("closed run accepted a message"); waiting.push(message); },
    stop() { stop = true; },
    inbox: async (final: boolean) => {
      drains.push(final);
      const messages = waiting.splice(0), stopped = stop;
      stop = false;
      if ((final && !messages.length) || stopped) closed = true;
      return { messages, stop: stopped };
    },
  };
}
const aiTool = (name: string, args: object, id: string = crypto.randomUUID()) =>
  new AIMessage({ content: "", tool_calls: [{ id, name, args, type: "tool_call" }] });
const boot: Boot = { max_turns: 12, definition: { name: "Steering test", memory: { enabled: true } }, tool_descriptors: [
  { name: "call_action", description: "Execute a business action", inputSchema: { type: "object", properties: { input: { type: "object" } }, required: ["input"] } },
  { name: "reply_now", description: "Send a short message now", inputSchema: { type: "object", properties: { text: { type: "string" } }, required: ["text"] } },
] };
const humans = (messages: BaseMessage[]) => messages.filter(isHumanMessage).map(m => m.text);

test("a correction sent during an action reaches the next step, which updates the saved draft", async () => {
  const box = mailbox(), calls: { name: string; args: any }[] = [];
  const model = new ScriptModel([
    () => aiTool("call_action", { input: { action: "create_quote", area: 12 } }),
    messages => {
      const correction = messages.at(-1)!;
      expect(isHumanMessage(correction)).toBe(true);
      expect(correction.id).toBe("wamid.correction");
      expect(correction.text).toContain("non, 15 m²");
      expect(correction.text).toContain("while you were working");
      expect(correction.text).toContain("instead of creating");
      return aiTool("reply_now", { text: "C'est noté, je passe la cuisine à 15 m²." });
    },
    () => aiTool("call_action", { input: { action: "update_quote", area: 15 } }),
    () => new AIMessage("Brouillon mis à jour : cuisine 15 m²."),
  ]);
  const bridge: Bridge = { state: database(), chunk() {}, inbox: box.inbox, callTool: async (name, args: any) => {
    calls.push({ name, args });
    if (args.input?.action === "create_quote") box.send({ id: "wamid.correction", content: "non, 15 m²" });
    return { saved: true };
  } };
  const result = await runHarness(boot, { invoke_id: "fix", session_id: "fix", message: "devis Dupont, cuisine 12 m²" }, model, bridge, "");
  expect(result.response).toBe("Brouillon mis à jour : cuisine 15 m².");
  expect(calls.map(c => c.name)).toEqual(["call_action", "reply_now", "call_action"]);
  expect(calls[2].args.input).toEqual({ action: "update_quote", area: 15 });
  expect(box.closed).toBe(true);
});

test("a message that arrives while the final answer is generated reopens the work instead of being dropped", async () => {
  const box = mailbox();
  const model = new ScriptModel([
    () => { box.send({ id: "wamid.late", content: "ajoute la peinture" }); return new AIMessage("Devis prêt."); },
    messages => {
      expect(humans(messages).at(-1)).toContain("ajoute la peinture");
      return new AIMessage("Devis prêt, peinture ajoutée.");
    },
  ]);
  const result = await runHarness(boot, { invoke_id: "late", session_id: "late", message: "devis Dupont" }, model,
    { state: database(), chunk() {}, inbox: box.inbox, callTool: async () => ({ saved: true }) }, "");
  expect(result.response).toBe("Devis prêt, peinture ajoutée.");
  expect(model.seen).toHaveLength(2);
  expect(box.drains.filter(final => final)).toHaveLength(2);
  expect(box.closed).toBe(true);
});

test("stop requested while the model plans an action ends the work without running it, and the next turn sees why", async () => {
  const box = mailbox(), state = database(), calls: unknown[] = [];
  const model = new ScriptModel([() => { box.stop(); return aiTool("call_action", { input: { action: "create_quote" } }, "planned"); }]);
  const bridge: Bridge = { state, chunk() { throw new Error("a stopped turn sends no answer"); }, inbox: box.inbox,
    callTool: async (...args) => { calls.push(args); return { saved: true }; } };
  const result = await runHarness(boot, { invoke_id: "stop", session_id: "stop", message: "devis Dupont" }, model, bridge, "");
  expect(result).toMatchObject({ response: "", stopped: true });
  expect(calls).toEqual([]);
  const next = new ScriptModel([messages => {
    const answer = messages.filter(isToolMessage).find(m => m.tool_call_id === "planned")!;
    expect(answer.text).toContain("Nothing was executed");
    expect(JSON.stringify(messages)).not.toContain("Execution was interrupted");
    return new AIMessage("Rien n'a été créé.");
  }]);
  const after = mailbox();
  expect((await runHarness(boot, { invoke_id: "after", session_id: "stop", message: "où en est-on ?" }, next,
    { ...bridge, inbox: after.inbox, chunk() {} }, "")).response).toBe("Rien n'a été créé.");
});

test("stop requested during an action lets that action finish and calls the model no further", async () => {
  const box = mailbox(), finished: string[] = [];
  const model = new ScriptModel([() => aiTool("call_action", { input: { action: "create_quote" } })]);
  const result = await runHarness(boot, { invoke_id: "during", session_id: "during", message: "devis Dupont" }, model,
    { state: database(), chunk() {}, inbox: box.inbox, callTool: async () => {
      box.stop();
      await new Promise(resolve => setTimeout(resolve, 5));
      finished.push("create_quote");
      return { saved: true };
    } }, "");
  expect(result).toMatchObject({ response: "", stopped: true });
  expect(finished).toEqual(["create_quote"]);
  expect(model.seen).toHaveLength(1);
});

test("a photo sent during the work is preserved in the workspace before the model reads the message", async () => {
  const box = mailbox(), state = database();
  const plan = Buffer.from("kitchen plan: 15 m2");
  const model = new ScriptModel([
    () => { box.send({ id: "wamid.photo", content: "voici le plan", files: [{
      name: "plan.txt", content_type: "text/plain", file_id: "11111111-1111-4111-8111-111111111111",
      url: `data:text/plain;base64,${plan.toString("base64")}` }] }); return aiTool("ls", { path: "/" }); },
    messages => {
      const text = humans(messages).at(-1)!;
      expect(text).toContain("voici le plan");
      expect(text).toContain("/attachments/11111111-1111-4111-8111-111111111111/plan.txt");
      return aiTool("read_file", { file_path: "/attachments/11111111-1111-4111-8111-111111111111/plan.txt" });
    },
    messages => {
      expect(messages.at(-1)!.text).toContain("kitchen plan");
      return new AIMessage("Plan lu.");
    },
  ]);
  const result = await runHarness(boot, { invoke_id: "photo", session_id: "photo", message: "devis Dupont" }, model,
    { state, chunk() {}, inbox: box.inbox, callTool: async () => ({}) }, "");
  expect(result.response).toBe("Plan lu.");
});
