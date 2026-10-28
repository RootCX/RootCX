# Core-managed agent harnesses

Agents can select a Core-managed runtime in `agent.json`:

```json
{
  "name": "Shappy",
  "harness": { "type": "deepagents" },
  "systemPrompt": "./agent/system.md",
  "memory": { "enabled": true },
  "limits": { "maxTurns": 60 },
  "supervision": { "mode": "autonomous" }
}
```

Omitting `harness` preserves the existing application entrypoint. Opting in replaces
the agent loop with the Core resource `agent-harness/index.js`; ordinary action,
job and lifecycle workers keep the app entrypoint. Applications no
longer need to bundle their own copy of LangChain for that loop. Existing app
business actions belong in their ordinary app workers. Model selection remains
Core's default LLM configuration (RootCX gateway, Anthropic, OpenAI or Bedrock).

The initial adapter pins Deep Agents JS 1.16.0. Its standard middleware supplies
virtual filesystem tools, subagents, context summarization and large result
offloading. Todo planning is enabled explicitly. Only app-authored
`agent/skills/**` are available through progressive skill loading.
An optional `pinnedSkills: ["skill-name"]` in `agent.json` supplies required,
app-authored skills through Deep Agents' native pinning middleware before each
invocation, including isolated subagents. Files live at
`agent/skills/<skill-name>/SKILL.md`; absent configured files fail before a model
call. Native `pinned_skill` messages make full-body loading observable. Re-pinning
on each invocation supplies the current release even in an existing conversation;
it does not enforce the model's business decisions. Keep task scope in the skill
body so unrelated requests do not adopt its workflow. Unpinned skills remain
on-demand; an absent `pinnedSkills` preserves existing behavior.
An app's `AGENTS.md` is read as app instructions. `/memories/AGENTS.md` can carry
user-specific preferences across conversations.

## Authority and state

The harness only reasons and coordinates. Business tools still cross the existing
`agent_tool_call` protocol: Core binds the responsible human, intersects their
permissions with the agent's grants and task scope, checks live revocation, applies
supervision/approval policies and dispatches the tool. Child agents keep their
existing Core delegation constraints. Internal Deep Agents subagents use the same
Core invocation and cannot call `invoke_agent`; they share the parent's model-call
budget. Business tools are not automatically retried after uncertain failures.

The new `agent_state` protocol accepts an active invocation ID, an operation and a
key. It accepts no user, app or session identity. Core derives the scope from the
admitted invocation and revalidates it on every operation. PostgreSQL stores state
under app, responsible human, origin/agent lineage, effective permission set and
session. The `memory` namespace spans that principal's conversations; checkpoints
and workspace files remain session-specific. An invocation cannot select another
person's session. Core also serializes turns for a session across worker processes.

Checkpoints preserve tool calls/results, messages and planning state; the file store
preserves original attachment bytes, extracted facts and pending questions. Binary
values are chunked below the IPC frame limit and published only after all chunks
exist. The Core-managed store needs no database credentials in the worker.
Business tool traces are persisted centrally, including for messaging channels.
Each graph step is checkpointed synchronously before the next step starts. A
worker restart therefore retains a completed tool result before another model call.
On a new turn, a tool call with no durable result receives an explicit unknown-outcome
result: it is not replayed and must not be represented as a cancelled business effect.
This does not provide exactly-once business execution. Apps still own idempotency;
a model must inspect current business state before deciding whether to retry.
Core closes the event relay and releases invocation authority and the session lock
on Done/Error, even if a dispatched tool still holds a sender. Already committed
business effects are not rolled back by disconnecting a conversation.
The existing user-facing transcript remains available to the chat UI.

`memory.enabled: false` uses ephemeral stores for that invocation. Switching an
existing custom agent to the managed harness can seed its textual transcript, but
cannot reconstruct old tool results or documents that were never persisted.

## Files and documents

Attachments keep their original Core `file_id` and get durable paths beneath
`/attachments/<file_id>/`. Models can revisit them after a clarification or a
worker restart. Business actions with `format: "file"` retain Core's existing
ownership-checked copy into the target application's storage.

Business instructions belong to the installed app's manifest (`describe_app`) or
the agent app's deployed instructions/skills. Core includes no quote-import playbook.
Instruction and skill files are mounted from the current release into ephemeral
stores; conversation checkpoints cannot keep a removed built-in skill active.
The runtime refreshes the middleware's instruction caches at each invocation.
The deployed `AGENTS.md` is also loaded by the native memory middleware for
isolated subagents, which do not inherit the parent's middleware. This loads only
`/instructions/AGENTS.md`, protected by inherited write-deny permissions; it does
not add personal `/memories/` contents to subagents.
Working files, attachments and user memories retain their normal persistence.

The final model answer is emitted as produced by the agent graph. There is no
additional editorial generation or sentence-count repair after business effects.
Response style belongs to app/host instructions; it is not a second execution
success condition. A completed answer at the model-call limit remains successful.

## Optional code execution

`ROOTCX_AGENT_SANDBOX=srt` opts a Linux Core into the `execute_sandbox` tool when the
sandbox resource is installed. It also requires `tool:execute_sandbox` within the
live delegated permission intersection. Deep Agents exposes it through its native
`execute` tool; the Core's approval, rate-limit and audit path still applies.

Each command runs in a fresh SRT/bubblewrap environment with no credentials and no
network. Inputs are explicit workspace text or files uploaded by the responsible
human to this agent. Python 3, Bun and Poppler are included in the Docker resource.
Only files beneath `outputs/` become Core artifacts, with returned `file_id` values.
Commands have time/output limits and output path checks. Application calculations
and business finalization must still use app actions.

**Code execution is disabled in the SHAPP local demo.** The host must permit nested
Linux namespaces and mounting a fresh `/proc` for the inner sandbox. Docker's
default outer profile rejects this on the tested host. A disposable-container test
passed file, environment and offline-network isolation with the appropriate outer
profile, but automatic approval review refused persisting that profile change on
the demo Core pod. No Core pod security profile was changed. This optional runtime
requires explicit approval and additional host qualification before activation.

## Local development

```sh
cd runtime/agent-harness
npm ci --ignore-scripts
npm run check
bun test
bun run build
```

Set `ROOTCX_AGENT_HARNESS` to the absolute `dist/index.js` path for a native Core,
or place the built file at `$ROOTCX_RESOURCES/agent-harness/index.js`. Docker builds
the pinned runtime with the Core image. The managed adapter has no dependency on
the app's `node_modules`.

Core integration tests: `agent_harness_test` (durability/isolation),
`managed_harness_boot_test` (deployment and runtime selection),
`managed_harness_recovery_test` (real adapter, approval/revocation, disconnect, worker
restart before approval and after commit with known/unknown result), and
`file_handoff_test` (attachment ownership and app handoff). Existing governance and
managed channel tests continue to exercise the unchanged business dispatch path.
The runtime suite also verifies subagent permission denials through the same bridge,
shared model-call budgets, memory-disabled operation and usage accounting across
nested calls without re-counting saved history. Provider callbacks supply per-run
usage rather than scanning only the parent graph's persisted messages.

## Future adapters and remaining engineering work

`AgentHarness` selects the runtime; the common boundary is Core's invocation, tool,
state and event protocol. A future adapter for Hermes or OpenClaw can implement
that boundary without receiving broader authority. Neither is implemented here.
They would need the same identity, revocation, approval and persistence tests.

Before a production rollout, qualify sandbox hosting separately, define checkpoint
retention and garbage collection of unreferenced content-addressed blobs, measure
long-conversation storage/latency, and decide which new agent scaffolds opt in.
Existing agents are not silently migrated. Deep Agents' own context policy currently
determines summarization; legacy `maxContextTokens`/`keepRecentMessages` are not
mapped to a second competing compaction mechanism.
