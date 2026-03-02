# RootCX AI Agents

## Philosophy

An agent is an **app with a brain**. Same manifest, same deploy, same RBAC — with a backend that talks to an LLM.

---

## Architecture

```
┌─────────────┐     SSE      ┌──────────────┐     IPC (stdin/stdout)     ┌──────────────────┐
│   Client     │◄────────────►│  Rust Core   │◄──────────────────────────►│  Agent Backend    │
│  (Browser)   │   HTTP POST  │  (Axum)      │   JSON lines              │  (Bun/Node)       │
└─────────────┘              └──────┬───────┘                            └────────┬─────────┘
                                    │                                             │
                              ┌─────▼──────┐                              ┌───────▼────────┐
                              │  PostgreSQL │                              │  Direct LLM    │
                              │  (sessions, │                              │  (Anthropic,   │
                              │   RBAC)     │                              │  OpenAI,       │
                              └────────────┘                              │  Bedrock)      │
                                                                          └────────────────┘
```

Core passes LLM credentials to the agent via IPC Discover. The scaffold generates provider-specific code (ChatAnthropic, ChatOpenAI, ChatBedrockConverse) based on the user's choice.

**Invocation flow:**

1. Client sends `POST /api/v1/apps/{app_id}/agent/invoke` with a message
2. Core loads agent config from DB (registered at deploy from `agent.json`)
3. Core resolves system prompt, history, tool descriptors, and issues a short-lived JWT
4. Core sends `agent_invoke` IPC message to the worker process
5. LangGraph's `createReactAgent` calls the LLM directly, executes tools via IPC
6. `agent_chunk` messages stream back through IPC → SSE to the client
7. On completion, the session is persisted and a `done` event closes the stream

### Identity & Permissions

Every agent gets a **deterministic UUID** derived from its app ID — same identity across restarts, same audit trail. Permissions are derived from the app's `dataContract`: every entity grants full CRUD to the agent's role.

### Memory

When `memory.enabled` is true, conversations persist across invocations. On the next call with the same `session_id`, the full history is loaded and prepended to the LLM context.

### Supervision

| Mode | Behavior |
|------|----------|
| `autonomous` | All tool calls execute immediately |
| `supervised` | Policies define which tools need approval or have rate limits |
| `strict` | Every tool call requires explicit approval |

Policies support `requires: "approval"` and `rate_limit: { max, window }` per tool/entity.

### Crash Recovery

If a worker crashes, the supervisor restarts it with exponential backoff (0s → 2s → 4s → 8s). After 5 crashes within 60 seconds, the worker enters a `Crashed` state and stops restarting.

---

## Building an Agent

### Project structure

```
my-agent/
├── manifest.json              # Data contract (same as any app)
├── .rootcx/launch.json        # Pre-launch hooks
├── src/App.tsx                 # Chat UI (scaffolded)
└── backend/
    ├── agent.json             # Agent config (limits, memory, supervision)
    ├── agent/system.md        # System prompt
    ├── index.ts               # LangGraph agent + IPC bridge
    └── package.json           # @langchain/langgraph, @langchain/openai, zod
```

### backend/agent.json

```json
{
  "name": "Support Assistant",
  "systemPrompt": "./agent/system.md",
  "memory": { "enabled": true },
  "limits": { "maxTurns": 15, "maxContextTokens": 100000, "keepRecentMessages": 10 },
  "supervision": { "mode": "autonomous" }
}
```

### Deploy

On deploy, the platform:
1. Reads `agent.json` and registers the agent in the DB
2. Creates RBAC role and system user
3. Starts the worker process

---

## API

### Invoke

```
POST /api/v1/apps/{app_id}/agent/invoke
Authorization: Bearer <user_jwt>

{ "message": "...", "session_id": "optional-uuid" }
```

Response is an **SSE stream**: `chunk`, `tool_call_started`, `tool_call_completed`, `approval_required`, `done`, `error` events.

### Sessions

```
GET /api/v1/apps/{app_id}/agent/sessions
GET /api/v1/apps/{app_id}/agent/sessions/{session_id}
GET /api/v1/apps/{app_id}/agent/sessions/{session_id}/events
```

### Approvals

```
GET  /api/v1/apps/{app_id}/agent/approvals
POST /api/v1/apps/{app_id}/agent/approvals/{approval_id}
{ "action": "approve" | "reject", "reason": "optional" }
```

### Agent Info

```
GET /api/v1/apps/{app_id}/agent
```

---

## Tools

Agents call tools via IPC — Core executes them server-side with RBAC enforcement.

| Tool | Purpose |
|------|---------|
| `query_data` | Read from any entity (filters, sorting, pagination) |
| `mutate_data` | Create, update, or delete records |
| `list_apps` | Discover installed apps and their entities |
| `describe_app` | Get full data contract of any app |
| `browser` | Navigate, click, type, screenshot web pages |

---

## LLM Providers

Configured via platform secrets (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, or `AWS_BEARER_TOKEN_BEDROCK`). Core passes credentials to the agent at startup via IPC. The scaffold generates provider-specific LangChain code (ChatAnthropic, ChatOpenAI, ChatBedrockConverse).
