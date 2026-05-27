// Minimal agent fixture for integration testing.
// v1 IPC protocol: uses readline on stdin.
// Exercises: discover handshake, agent_invoke, agent_tool_call (query_data).
// After sending the tool call, waits for agent_tool_result, then sends agent_done.

const { createInterface } = require("readline");
const rl = createInterface({ input: process.stdin });
const write = (m) => process.stdout.write(JSON.stringify(m) + "\n");

let invokeId = null;

rl.on("line", (l) => {
  let m;
  try { m = JSON.parse(l); } catch { return; }

  if (m.type === "discover") {
    write({ type: "discover", protocol: 2 });
    return;
  }

  if (m.type === "agent_invoke") {
    invokeId = m.invoke_id;
    write({
      type: "agent_tool_call",
      invoke_id: invokeId,
      call_id: "call-1",
      tool_name: "query_data",
      args: { entity: "tasks" },
    });
    return;
  }

  if (m.type === "agent_tool_result") {
    const hasError = !!m.error;
    write({
      type: "agent_done",
      invoke_id: invokeId,
      response: hasError ? "DENIED:" + m.error : "OK:tool_executed",
      tokens: 0,
    });
    return;
  }
});
