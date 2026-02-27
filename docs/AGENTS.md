# RootCX AI Agents

## Philosophy

RootCX agents are **autonomous AI workers that live inside your application**. They aren't chatbots — they are first-class actors with their own identity, permissions, and persistent memory, operating on real business data through the same API your app uses.

Three core principles guide the design:

1. **Declarative over imperative** — Define *what* an agent can do in `manifest.json`. The platform handles *how*.
2. **Convention over configuration** — Agents get all registered tools automatically. RBAC permissions are derived from the app's data contract.
3. **Isolated execution** — Each agent runs in its own process, communicates via IPC, and can crash without taking down the runtime.

---

## How It Works

### Architecture

```
┌─────────────┐     SSE      ┌──────────────┐     IPC (stdin/stdout)     ┌──────────────────┐
│   Client     │◄────────────►│  Rust Core   │◄──────────────────────────►│  TS Agent Runtime │
│  (Browser)   │   HTTP POST  │  (Axum)      │   JSON lines              │  (Bun/Node)       │
└─────────────┘              └──────┬───────┘                            └────────┬─────────┘
                                    │                                             │
                              ┌─────▼──────┐                              ┌───────▼────────┐
                              │  PostgreSQL │                              │  LangGraph +   │
                              │  (sessions, │                              │  LLM Provider  │
                              │   RBAC)     │                              │  (Claude, GPT, │
                              └────────────┘                              │   Bedrock)     │
                                                                          └────────────────┘
```

**Invocation flow:**

1. Client sends `POST /api/v1/apps/{app_id}/agent/invoke` with a message
2. Core loads the agent config, system prompt, conversation history, and resolves which tools are available
3. Core spawns (or reuses) the agent's TypeScript worker process
4. The TS runtime builds the LLM provider, loads the LangGraph, and starts reasoning
5. As the LLM generates tokens, `agent_chunk` messages stream back through IPC → SSE to the client
6. When the agent needs data, it calls tools (query_data, mutate_data, browser...) that execute server-side
7. On completion, the session is persisted and a `done` event closes the stream

### Identity & Permissions

Every agent gets a **deterministic UUID** derived from its app ID. This UUID is a system user in the RBAC layer — same identity across restarts, same audit trail.

Permissions are derived automatically from the app's `dataContract`. Every entity in the contract grants full CRUD permissions to the agent's RBAC role. For example, if `dataContract` contains `tickets` and `customers`, the agent gets `tickets.create`, `tickets.read`, `tickets.update`, `tickets.delete`, `customers.create`, `customers.read`, `customers.update`, `customers.delete`.

All registered tools (query_data, mutate_data, browser, list_apps, describe_app, etc.) are available to every agent — tools are platform capabilities, not agent properties.

### Memory

When `memory.enabled` is true, conversations persist across invocations. Each session accumulates messages in a JSONB column. On the next call with the same `session_id`, the full history is loaded and prepended to the LLM context.

### Crash Recovery

The Rust supervisor monitors the agent process. If it crashes:
- Exponential backoff: 0s → 2s → 4s → 8s...
- After 5 crashes in 60 seconds, the agent is marked `Crashed` and stops restarting
- Stop commands can interrupt backoff (no stuck processes)

---

## Building an Agent

### 1. Define in manifest.json

```json
{
  "appId": "support-bot",
  "name": "Support Bot",
  "dataContract": [
    {
      "entityName": "tickets",
      "fields": [
        { "name": "title", "type": "text", "required": true },
        { "name": "status", "type": "text", "enumValues": ["open", "resolved", "escalated"] },
        { "name": "customer_email", "type": "text", "required": true },
        { "name": "resolution", "type": "text" }
      ]
    }
  ],
  "agent": {
    "name": "Support Assistant",
    "provider": { "type": "anthropic", "model": "claude-sonnet-4-6", "api_key": "${ANTHROPIC_API_KEY}" },
    "systemPrompt": "./agent/system.md",
    "memory": { "enabled": true },
    "limits": { "maxTurns": 15 }
  }
}
```

The `api_key` field uses `${SECRET_NAME}` syntax — resolved at runtime from the platform secret store, never stored in config.

### 2. Write the system prompt

**backend/agent/system.md:**

```markdown
You are the Support Assistant for Acme Corp.

## Data
You have access to the tickets entity:
- title (text): Short description of the issue
- status (text): open | resolved | escalated
- customer_email (text): The customer's email
- resolution (text): How the issue was resolved

## Workflow
1. Query existing tickets to check for duplicates
2. Create a new ticket if none exists
3. Investigate and attempt resolution
4. Update the ticket with your findings

## Rules
- Always check for existing tickets before creating new ones
- Set status to "escalated" if you cannot resolve within 3 tool calls
- Never promise timelines or refunds
```

### 3. (Optional) Custom graph

By default, agents use a ReAct loop: think → call tools → think → ... → respond. For more complex workflows, provide `backend/agent/graph.ts`:

```typescript
import { StateGraph, MessagesAnnotation } from "@langchain/langgraph";
import { ToolNode } from "@langchain/langgraph/prebuilt";
import type { BaseChatModel } from "@langchain/core/language_models/chat_models";
import type { StructuredToolInterface } from "@langchain/core/tools";

export default function buildGraph(model: BaseChatModel, tools: StructuredToolInterface[]) {
    const bound = model.bindTools(tools);
    const toolNode = new ToolNode(tools);

    async function agent(state: typeof MessagesAnnotation.State) {
        return { messages: [await bound.invoke(state.messages)] };
    }

    function route(state: typeof MessagesAnnotation.State) {
        const last = state.messages.at(-1) as { tool_calls?: unknown[] } | undefined;
        return last?.tool_calls?.length ? "tools" : "__end__";
    }

    return new StateGraph(MessagesAnnotation)
        .addNode("agent", agent)
        .addNode("tools", toolNode)
        .addEdge("__start__", "agent")
        .addConditionalEdges("agent", route)
        .addEdge("tools", "agent")
        .compile();
}
```

The function receives the LLM instance and all registered tools. You wire them however you want — multi-agent handoffs, conditional branches, human-in-the-loop checkpoints.

### 4. Entry point

**backend/index.ts:**

```typescript
import "@rootcx/agent-runtime";
```

That's it. The runtime handles IPC, tool bridging, and graph execution.

### 5. Deploy

The app is installed via the Studio or the REST API. On install, the platform:
- Creates the agent's DB tables (from `dataContract`)
- Registers the agent in `rootcx_system.agents`
- Creates an RBAC role and system user
- Starts the worker process

---

## Using an Agent (End User / Production)

### Invoke

```
POST /api/v1/apps/support-bot/agent/invoke
Authorization: Bearer <user_jwt>
Content-Type: application/json

{
  "message": "Customer sarah@acme.com reports order #4521 hasn't arrived after 2 weeks",
  "session_id": "optional-uuid-for-continuity"
}
```

Response is an **SSE stream**:

```
event: chunk
data: {"delta":"Let me check","session_id":"abc-123"}

event: chunk
data: {"delta":" for existing tickets...","session_id":"abc-123"}

event: done
data: {"response":"I found an existing ticket...","session_id":"abc-123","tokens":342}
```

If `session_id` is omitted, a new session is created. Pass the same ID to continue a conversation.

### List Sessions

```
GET /api/v1/apps/support-bot/agent/sessions
```

Returns all sessions ordered by last activity.

### Get Session Detail

```
GET /api/v1/apps/support-bot/agent/sessions/{session_id}
```

Returns the full message history.

### Get Agent Info

```
GET /api/v1/apps/support-bot/agent
```

Returns the agent's name, description, and config.

---

## Available Tools

Agents don't call APIs directly. They use **tools** — server-side functions that respect RBAC.

| Tool | Purpose |
|------|---------|
| `query_data` | Read from any entity (filters, sorting, pagination) |
| `mutate_data` | Create, update, or delete records |
| `list_apps` | Discover installed apps and their entities |
| `describe_app` | Get full data contract of any app |
| `browser` | Navigate, click, type, screenshot web pages |

### query_data example (what the LLM sends)

```json
{
  "entity": "tickets",
  "where": { "customer_email": "sarah@acme.com", "status": { "$ne": "resolved" } },
  "orderBy": "created_at",
  "order": "desc",
  "limit": 10
}
```

### mutate_data example

```json
{
  "entity": "tickets",
  "action": "create",
  "data": { "title": "Missing order #4521", "status": "open", "customer_email": "sarah@acme.com" }
}
```

---

## LLM Providers

Agents support three providers, configured in the manifest:

| Provider | Config | Auth |
|----------|--------|------|
| Anthropic | `{ "type": "anthropic", "model": "claude-sonnet-4-6" }` | `api_key` or `${SECRET}` |
| OpenAI | `{ "type": "openai", "model": "gpt-4o" }` | `api_key` or `${SECRET}` |
| Bedrock | `{ "type": "bedrock", "model": "us.anthropic.claude-sonnet-4-6", "region": "us-east-1" }` | IAM role (no key) |

Secret references (`${ANTHROPIC_API_KEY}`) are resolved at invocation time from the platform secret store — keys never reach the agent process in config.

---

## Key Takeaways

- **Agents are apps.** They live alongside your data, use the same schema, same auth, same audit log.
- **Declare, don't code.** Manifest defines identity, permissions, provider, memory. The graph is optional.
- **Tools are the API.** Agents interact with data through the same query/mutate interface as the rest of the platform.
- **Memory is opt-in.** Stateless by default. Enable `memory.enabled` for persistent conversations.
- **Isolation is real.** Separate process, separate identity, separate permissions. A broken agent can't affect others.
