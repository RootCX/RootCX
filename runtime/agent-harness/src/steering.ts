import { createMiddleware } from "langchain";
import { HumanMessage, ToolMessage, isAIMessage, type BaseMessage } from "@langchain/core/messages";

export type InboxFile = { name: string; content_type: string; url: string; file_id: string };
export type InboxMessage = { id: string; content: string; files?: InboxFile[] };
/** Core's inbox for the running turn. A final drain that finds nothing closes it atomically. */
export type Inbox = (final: boolean) => Promise<{ messages: InboxMessage[]; stop: boolean }>;

const NOT_RUN = "Not run: the person asked to stop before this action started. Nothing was executed for this call.";

function frame(content: string, references: string[]) {
  return [
    "The person sent this message while you were working on their request:",
    content,
    references.length ? `Files available in your durable workspace:\n${references.join("\n")}` : "",
    "Take it into account before your next action. Writes already made for this request may be saved: correct or update those records instead of creating new ones. Unless your next message is your final answer, first confirm in one short sentence that you took it into account, with reply_now when that tool is available, then continue.",
  ].filter(Boolean).join("\n\n");
}

/**
 * Reads messages the person sends while the main agent works, at the boundaries
 * LangChain gives between steps, the way deepagents' async subagent middleware
 * reports finished tasks. A message is never placed between an action request and
 * its result; one that arrives while actions are pending waits for the next step.
 * Stopping is cooperative: an action already running finishes, none starts after.
 */
export function steering(inbox: Inbox, preserve: (files: InboxFile[]) => Promise<string[]>) {
  const run = { stopped: false };
  let held: BaseMessage[] = [];
  const take = async (final: boolean) => {
    const { messages, stop } = await inbox(final);
    const received = await Promise.all(messages.map(async message => new HumanMessage({
      id: message.id,
      content: frame(message.content, message.files?.length ? await preserve(message.files) : []),
    })));
    const ready = [...held, ...received];
    held = [];
    return { ready, stop };
  };
  const middleware = createMiddleware({
    name: "RootCXSteering",
    beforeModel: {
      canJumpTo: ["end"],
      hook: async () => {
        const { ready, stop } = await take(false);
        if (stop) { run.stopped = true; return { messages: ready, jumpTo: "end" as const }; }
        return ready.length ? { messages: ready } : undefined;
      },
    },
    afterModel: {
      canJumpTo: ["model", "end"],
      hook: async (state: { messages: BaseMessage[] }) => {
        const last = state.messages.at(-1);
        if (!last || !isAIMessage(last)) return;
        const calls = last.tool_calls ?? [];
        const { ready, stop } = await take(calls.length === 0);
        if (stop) {
          run.stopped = true;
          const skipped = calls.map(call => new ToolMessage({ tool_call_id: call.id!, name: call.name, status: "error", content: NOT_RUN }));
          return { messages: [...skipped, ...ready], jumpTo: "end" as const };
        }
        if (calls.length) { held = ready; return; }
        // A final answer is not final while the person is still talking: answer them too.
        return ready.length ? { messages: ready, jumpTo: "model" as const } : undefined;
      },
    },
  });
  return { middleware, run };
}
