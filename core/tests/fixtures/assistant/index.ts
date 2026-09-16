// Deterministic assistant for Core integration tests. Core still performs the
// real agent registration, worker discovery and supervised invocation path.
// No package install, external release, provider credentials or LLM required.
import { createInterface } from "node:readline";

const send = (message: unknown) => process.stdout.write(JSON.stringify(message) + "\n");
createInterface({ input: process.stdin }).on("line", (line) => {
  const message = JSON.parse(line);
  if (message.type === "discover") {
    send({ type: "discover", protocol: 2 });
  } else if (message.type === "agent_invoke") {
    send({
      type: "agent_done",
      invoke_id: message.invoke_id,
      response: `received: ${message.message}`,
      tokens: 0,
    });
  }
});
