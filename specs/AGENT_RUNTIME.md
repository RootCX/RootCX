# RootCX Agent Runtime — Development Specification

> **Vision**: A company opens Studio, describes an AI agent in natural language,
> Forge generates everything, they deploy. The agent runs on Core — same RBAC,
> same audit, same data, same infrastructure. Pure code. Auditable. Self-hostable.

---

## Apps vs. Agents

RootCX Core manages **deployable units**. Today those are apps. With this spec, they can also be agents. Same manifest, same deploy pipeline, same infrastructure — but different purpose.

| | App | Agent |
|---|---|---|
| **Purpose** | Business application (UI + backend + data) | Autonomous AI worker (LLM + tools + data) |
| **Has a frontend** | Yes (React) | No |
| **Has a backend worker** | Yes (RPC handlers in Bun) | Yes (LangGraph in Bun) |
| **Has a dataContract** | Yes — the app's business data | Yes — the agent's own persistent storage |
| **Access control** | `permissions` — roles for human users | `access` — inside agent definition |
| **Deploy** | `rootcx deploy` | `rootcx deploy` — same pipeline |
| **Manifest** | `appId`, `dataContract`, `permissions` | `appId`, `dataContract`, `agents` (with `access` inside) |

An agent's `dataContract` is **its own data** — what the agent needs to persist to do its job. A sales prospector stores leads and research notes. A compliance checker stores audit findings. The agent reads and writes this data through the same CRUD API, governed by the same RBAC.

---

## Execution Model

An agent runs **exactly like an app backend** — it's a Bun process managed by Core. There is no separate agent infrastructure. The existing worker supervision pipeline handles everything.

```
┌─ Core (Rust) ─────────────────────────────────────────────────────┐
│                                                                     │
│  WorkerManager                                                      │
│  ├─ App "inventory"   → Bun worker (RPC handlers)     ← stdin/stdout JSONL
│  ├─ App "ticketing"   → Bun worker (RPC handlers)     ← stdin/stdout JSONL
│  └─ Agent "prospector"→ Bun worker (LangGraph runtime) ← stdin/stdout JSONL
│       │                     │                                       │
│       │  Same supervisor:   │                                       │
│       │  ─ crash recovery (5 crashes / 60s → give up)               │
│       │  ─ exponential backoff on restart                           │
│       │  ─ graceful shutdown via IPC                                │
│       │  ─ process group kill                                       │
│       │  ─ stderr/stdout log capture                                │
│                                                                     │
└─────────────────────────────────────────────────────────────────────┘
```

**What's identical between an app worker and an agent worker:**

| | App worker | Agent worker |
|---|---|---|
| **Spawned by** | `WorkerManager` | `WorkerManager` — same code path |
| **Runtime** | Bun | Bun |
| **Supervised by** | `supervisor_loop` in `worker.rs` | Same `supervisor_loop` — no changes |
| **IPC protocol** | JSONL over stdin/stdout | Same JSONL — new message types added |
| **Crash recovery** | `MAX_CRASHES=5`, `CRASH_WINDOW=60s`, exponential backoff | Identical — inherited from supervisor |
| **Env vars** | `ROOTCX_APP_ID`, `ROOTCX_RUNTIME_URL` | Same vars — agent reads them |
| **Secrets** | Injected as env vars by `WorkerManager` | Same — LLM API keys injected the same way |
| **Process lifecycle** | Start → Discover handshake → Running → Shutdown | Identical lifecycle |

**What's different:**

| | App worker | Agent worker |
|---|---|---|
| **Entry point** | `backend/index.ts` — loads RPC handlers | `runtime/agent/index.ts` — loads LangGraph |
| **Inbound messages** | `Rpc { method, params }`, `Job { payload }` | New: `AgentInvoke { agent_id, session_id, message, ... }` |
| **Outbound messages** | `RpcResponse { result }`, `JobResult { result }` | New: `AgentChunk { delta }`, `AgentDone { response }`, `AgentError { error }` |
| **Discover handshake** | `capabilities: []` | `capabilities: ["agent"]` |

The supervisor doesn't know or care that it's running an agent. It spawns a Bun process, watches its health, restarts it on crash, and relays IPC messages. The agent-specific logic lives entirely in the TypeScript entry point and in Core's `AgentExtension` routes (which translate HTTP requests into IPC messages and stream the results back as SSE).

**Sequence: invoking an agent**

```
Client                    Core (Rust)                    Bun Worker (Agent)
  │                          │                                │
  │  POST /invoke            │                                │
  │  { message: "..." }      │                                │
  │─────────────────────────►│                                │
  │                          │                                │
  │  SSE stream opened       │  stdin: AgentInvoke            │
  │◄─────────────────────────│───────────────────────────────►│
  │                          │                                │
  │                          │  stdout: AgentChunk { delta }  │
  │  event: chunk            │◄───────────────────────────────│
  │  data: { delta }         │                                │
  │◄─────────────────────────│                                │
  │                          │                                │
  │                          │  (agent calls tools, loops)    │
  │  event: chunk            │                                │
  │◄─────────────────────────│  stdout: AgentChunk { delta }  │
  │                          │◄───────────────────────────────│
  │                          │                                │
  │                          │  stdout: AgentDone { response }│
  │  event: done             │◄───────────────────────────────│
  │  data: { response }      │                                │
  │◄─────────────────────────│                                │
  │  SSE stream closed       │                                │
```

This is the same pattern as an app RPC call (`Rpc` in → `RpcResponse` out), but with streaming chunks instead of a single response.

---

## The User Experience

```
┌─ Studio ─────────────────────────────────────────────────────────┐
│                                                                   │
│  User opens AI Forge and types:                                   │
│                                                                   │
│  "Build me a sales prospector agent. It should search the web     │
│   for leads, store research notes, score prospects, and remember  │
│   what we discussed across sessions."                             │
│                                                                   │
│  ┌─ Forge generates ──────────────────────────────────────────┐  │
│  │                                                             │  │
│  │  manifest.json                                              │  │
│  │  ├─ appId: "sales-prospector"                               │  │
│  │  ├─ dataContract: leads, research_notes                     │  │
│  │  └─ agents.prospector: model, memory, limits, access        │  │
│  │                                                             │  │
│  │  agents/prospector/system.md                                │  │
│  │  └─ System prompt: research instructions, scoring rubric    │  │
│  │                                                             │  │
│  │  agents/prospector/graph.ts                                 │  │
│  │  └─ LangGraph: research → enrich → score → summarize       │  │
│  │                                                             │  │
│  └─────────────────────────────────────────────────────────────┘  │
│                                                                   │
│  Code appears in the editor. User reviews, tweaks prompt.         │
│  Clicks Deploy. Done.                                             │
│                                                                   │
│  Core reads the manifest:                                         │
│  ─ Creates tables (leads, research_notes)                         │
│  ─ Creates agent role + RBAC policies from access list            │
│  ─ Registers the agent                                            │
│  ─ Starts the agent worker                                        │
│                                                                   │
│  Agent is live.                                                   │
└───────────────────────────────────────────────────────────────────┘
```

The experience is identical to building an app. Forge generates code → user reviews → deploy. The difference is what Forge generates: a LangGraph + system prompt instead of a React UI + backend handlers.

---

## Manifest Specification

The manifest gains one new top-level field: `agents`. No `permissions` needed for a standalone agent — the agent's access rules live inside its own definition via the `access` key.

```
manifest.json
├─ appId, name, version          identity
├─ dataContract                  what data exists
└─ agents
   └─ prospector
      ├─ name, model, graph...   how the agent behaves
      └─ access                  what the agent can do
```

`permissions` (top-level) remains reserved for human roles — used by apps, optional for agents if a human dashboard is added later.

### Full manifest example: Sales Prospector Agent

```json
{
    "appId": "sales-prospector",
    "name": "Sales Prospector",
    "version": "0.1.0",
    "description": "AI agent that researches leads, enriches profiles, and scores prospects",

    "dataContract": [
        {
            "entityName": "leads",
            "fields": [
                { "name": "name", "type": "text", "required": true },
                { "name": "company", "type": "text" },
                { "name": "title", "type": "text" },
                { "name": "email", "type": "text" },
                { "name": "linkedin_url", "type": "text" },
                { "name": "score", "type": "number" },
                { "name": "status", "type": "text", "enumValues": ["new", "researched", "qualified", "disqualified"] },
                { "name": "summary", "type": "text" }
            ]
        },
        {
            "entityName": "research_notes",
            "fields": [
                { "name": "lead_id", "type": "entity_link", "references": { "entity": "leads", "field": "id" } },
                { "name": "source", "type": "text", "required": true },
                { "name": "content", "type": "text", "required": true },
                { "name": "category", "type": "text", "enumValues": ["company_info", "person_info", "news", "financial"] }
            ]
        }
    ],

    "agents": {
        "prospector": {
            "name": "Sales Prospector",
            "description": "Researches leads, enriches profiles, scores prospects",
            "model": "claude-sonnet-4-20250514",
            "systemPrompt": "./agents/prospector/system.md",
            "graph": "./agents/prospector/graph.ts",
            "memory": { "enabled": true },
            "limits": { "maxTurns": 20 },
            "access": [
                { "entity": "leads", "actions": ["read", "create", "update"] },
                { "entity": "research_notes", "actions": ["read", "create"] },
                { "entity": "tool:web_search", "actions": ["use"] },
                { "entity": "tool:web_fetch", "actions": ["use"] },
                { "entity": "app:crm/contacts", "actions": ["read"] }
            ]
        }
    }
}
```

**Read the `access` list like a sentence — it says everything the agent can do:**

| Access rule | In plain language |
|-------------|-------------------|
| `entity: "leads", actions: ["read", "create", "update"]` | It can view, create, and update leads |
| `entity: "research_notes", actions: ["read", "create"]` | It can view and create research notes |
| `entity: "tool:web_search", actions: ["use"]` | It can search the web (Google, LinkedIn, etc.) |
| `entity: "tool:web_fetch", actions: ["use"]` | It can fetch and read web pages |
| `entity: "app:crm/contacts", actions: ["read"]` | It can read contacts from the CRM app |

**What's NOT listed = what the agent CANNOT do:**
- No `delete` action anywhere → the agent can never delete data
- No `tool:send_email` → the agent can't send emails
- No `app:crm/contacts` with `create` → it can read CRM contacts but not modify them

One block. Complete picture.

### `agents` section schema

```typescript
{
    [agentId: string]: {
        name: string;                     // Human-readable display name
        description?: string;             // What this agent does
        model?: string;                   // LLM model ID (default: "claude-sonnet-4-20250514")
        systemPrompt?: string;            // Relative path to .md file (default: "./agents/{id}/system.md")
        graph?: string;                   // Relative path to .ts file (default: null → built-in ReAct)
        memory?: {
            enabled: boolean;             // true = persist session messages + load on resume
        };
        limits?: {
            maxTurns?: number;            // Max agent↔tools loop iterations (default: 10)
        };
        access: Array<{
            entity: string;               // Collection name, "tool:{name}", or "app:{appId}/{entity}"
            actions: string[];            // ["read","create","update","delete"] or ["use"] for tools
        }>;
    }
}
```

**Conventions:**
- Agent IDs are kebab-case, unique within the deployment
- Core auto-creates the RBAC role `agent:{id}` from the `access` list — no manual role declaration
- The `systemPrompt` path is relative to the project root
- If `graph` is omitted, the agent uses the built-in ReAct loop
- Tools are determined from `access`: entries with `entity` starting with `tool:` → those tools get loaded into the LangGraph

### How `access` works under the hood

At install time, Core reads `agents.{id}.access` and translates it into standard RBAC artifacts:

1. Creates role `agent:prospector` in `rbac_roles`
2. For each access entry, creates a policy in `rbac_policies`:
   - `{ entity: "leads", actions: ["read", "create", "update"] }` → RBAC policy `(agent:prospector, leads, read/create/update)`
   - `{ entity: "tool:web_search", actions: ["use"] }` → stored as tool grant
   - `{ entity: "app:crm/contacts", actions: ["read"] }` → RBAC policy with cross-app flag

Same RBAC tables, same enforcement engine, same audit trail. The `access` list is just a cleaner way to declare it in the manifest.

**Three kinds of entity:**

| Entity pattern | What it means | Enforced by |
|---|---|---|
| `leads`, `research_notes` | Agent's own collections | CRUD route middleware (existing RBAC) |
| `tool:web_search`, `tool:web_fetch` | Platform tool access | Tool loader at agent startup |
| `app:crm/contacts` | Cross-app data access | CRUD route with cross-app auth check |

**How it works at runtime:**

1. Agent starts → Core reads access entries starting with `tool:` → loads those tools into the LangGraph
2. Agent calls `query_data({ entity: "leads" })` → Core checks `(agent:prospector, leads, read)` → **allowed**
3. Agent calls `mutate_data({ entity: "leads", action: "create", ... })` → Core checks `(agent:prospector, leads, create)` → **allowed**, audit logged
4. Agent calls `mutate_data({ entity: "leads", action: "delete", ... })` → Core checks `(agent:prospector, leads, delete)` → **denied**, not in access list
5. Agent calls `query_data({ app: "crm", entity: "contacts" })` → Core checks `(agent:prospector, app:crm/contacts, read)` → **allowed**, reads from CRM app's schema

**"Can the agent go on LinkedIn?"** → Yes, it has `tool:web_search` and `tool:web_fetch` in its access. It can search and fetch any URL. Domain-level scoping (restrict to only linkedin.com) is a future iteration.

**"Can the agent read CRM contacts?"** → Yes, `app:crm/contacts` with `read` is in its access list. Core routes the request to the CRM app's data and returns results. Read-only — no `create`/`update` listed.

**"Can the agent delete leads?"** → No. `delete` is not in the actions for `leads`. RBAC denies, audit logs.

### Platform tools catalog

| Entity in `access` | Tool loaded | What it does |
|---|---|---|
| `tool:web_search` | `web_search` | Search the internet via Brave Search or Tavily |
| `tool:web_fetch` | `web_fetch` | Fetch a URL and extract readable content |

`query_data` and `mutate_data` are auto-loaded when the agent has ANY data access entry (own collections or cross-app). `web_search` and `web_fetch` are loaded only when the corresponding `tool:` entry exists in `access`.

Every data tool call goes through Core's middleware: authentication → RBAC → execution → audit.

---

## Agent Implementation (TypeScript)

Agents are TypeScript, consistent with app workers. The agent runtime is a new package `@rootcx/agent-runtime` that runs inside Bun.

### Project structure

```
sales-prospector/
├── manifest.json
├── agents/
│   └── prospector/
│       ├── system.md              # System prompt (markdown)
│       └── graph.ts               # Optional: custom LangGraph
└── package.json                   # includes @rootcx/agent-runtime
```

No `backend/` folder. No `src/` folder. No frontend. Just the manifest, the agent code, and dependencies.

### The agent runtime package (`runtime/agent/`)

```
runtime/agent/
├── src/
│   ├── index.ts              # Entry point: IPC listener, agent dispatcher
│   ├── runner.ts             # Load config, build graph, execute, stream results
│   ├── default-graph.ts      # Built-in ReAct loop
│   ├── provider.ts           # LLM provider abstraction
│   ├── ipc.ts                # JSONL stdin/stdout protocol
│   └── tools/
│       ├── registry.ts       # Loads tools granted by agent policies
│       ├── query-data.ts
│       ├── mutate-data.ts
│       ├── web-search.ts
│       └── web-fetch.ts
├── package.json
└── tsconfig.json
```

Dependencies: `@langchain/langgraph`, `@langchain/anthropic`, `@langchain/openai`, `@langchain/core`, `zod`

### Entry point (`index.ts`)

```typescript
import { IpcReader, IpcWriter } from "./ipc";
import { runAgent } from "./runner";

const reader = new IpcReader(process.stdin);
const writer = new IpcWriter(process.stdout);

reader.on("agent_invoke", async (msg) => {
    try {
        await runAgent({
            agentId: msg.agent_id,
            sessionId: msg.session_id,
            message: msg.message,
            systemPrompt: msg.system_prompt,
            config: msg.config,
            history: msg.history,
            caller: msg.caller,
            writer,
        });
    } catch (err) {
        writer.send({
            type: "agent_error",
            session_id: msg.session_id,
            error: err instanceof Error ? err.message : String(err),
        });
    }
});

reader.on("discover", () => {
    writer.send({ type: "discover", capabilities: ["agent"] });
});
```

### Agent runner (`runner.ts`)

```typescript
import { ChatAnthropic } from "@langchain/anthropic";
import { ChatOpenAI } from "@langchain/openai";
import { HumanMessage, SystemMessage, BaseMessage } from "@langchain/core/messages";
import { buildDefaultGraph } from "./default-graph";
import { buildToolRegistry } from "./tools/registry";
import type { IpcWriter } from "./ipc";

interface RunAgentParams {
    agentId: string;
    sessionId: string;
    message: string;
    systemPrompt: string;
    config: AgentConfig;
    history: BaseMessage[];
    caller: { user_id: string; username: string } | null;
    writer: IpcWriter;
}

export async function runAgent(params: RunAgentParams) {
    const { agentId, sessionId, message, systemPrompt, config, history, caller, writer } = params;

    // 1. Build LLM provider
    const model = buildProvider(config.model);

    // 2. Tools come from access list: entity starting with "tool:" → tool name
    //    e.g. { entity: "tool:web_search" } → loads "web_search"
    //    query_data/mutate_data are auto-loaded when any data access entry exists
    const tools = buildToolRegistry(config._enabledTools, {
        appId: config._appId,
        agentId,
        runtimeUrl: process.env.ROOTCX_RUNTIME_URL!,
    });

    // 3. Load custom graph or use default ReAct
    let graph;
    if (config._graphAbsolutePath) {
        const custom = await import(config._graphAbsolutePath);
        graph = typeof custom.default === "function"
            ? custom.default(model, tools)   // custom graph receives model + tools
            : custom.default;
    } else {
        graph = buildDefaultGraph(model, tools);
    }

    // 4. Assemble messages: system prompt + session history + new message
    const messages: BaseMessage[] = [
        new SystemMessage(systemPrompt),
        ...history,
        new HumanMessage(message),
    ];

    // 5. Execute the graph with streaming
    const maxTurns = config.limits?.maxTurns ?? 10;
    let turns = 0;
    let finalResponse = "";

    const stream = await graph.stream(
        { messages },
        { recursionLimit: maxTurns * 2 },
    );

    for await (const event of stream) {
        if (++turns > maxTurns) {
            writer.send({
                type: "agent_error",
                session_id: sessionId,
                error: `Max turns (${maxTurns}) exceeded`,
            });
            return;
        }

        if (event.agent?.messages) {
            for (const msg of event.agent.messages) {
                if (msg.content && typeof msg.content === "string") {
                    finalResponse = msg.content;
                    writer.send({
                        type: "agent_chunk",
                        session_id: sessionId,
                        delta: msg.content,
                    });
                }
            }
        }
    }

    writer.send({
        type: "agent_done",
        session_id: sessionId,
        response: finalResponse,
        tokens: 0, // TODO: extract from provider response metadata
    });
}

function buildProvider(modelId: string | undefined) {
    const id = modelId ?? "claude-sonnet-4-20250514";

    if (id.startsWith("gpt-") || id.startsWith("openai/")) {
        return new ChatOpenAI({ model: id, apiKey: process.env.OPENAI_API_KEY, streaming: true });
    }

    return new ChatAnthropic({ model: id, apiKey: process.env.ANTHROPIC_API_KEY, streaming: true });
}
```

### Default ReAct graph (`default-graph.ts`)

The built-in graph. Used when an agent does not provide a custom `graph.ts`.

```typescript
import { StateGraph, MessagesAnnotation } from "@langchain/langgraph";
import { ToolNode } from "@langchain/langgraph/prebuilt";
import type { BaseChatModel } from "@langchain/core/language_models/chat_models";
import type { StructuredToolInterface } from "@langchain/core/tools";

export function buildDefaultGraph(model: BaseChatModel, tools: StructuredToolInterface[]) {
    const modelWithTools = model.bindTools(tools);
    const toolNode = new ToolNode(tools);

    async function callModel(state: typeof MessagesAnnotation.State) {
        const response = await modelWithTools.invoke(state.messages);
        return { messages: [response] };
    }

    function shouldContinue(state: typeof MessagesAnnotation.State) {
        const last = state.messages[state.messages.length - 1];
        return last.tool_calls?.length ? "tools" : "__end__";
    }

    return new StateGraph(MessagesAnnotation)
        .addNode("agent", callModel)
        .addNode("tools", toolNode)
        .addEdge("__start__", "agent")
        .addConditionalEdges("agent", shouldContinue)
        .addEdge("tools", "agent")
        .compile();
}
```

### Custom graph example (`agents/prospector/graph.ts`)

A multi-stage pipeline: research → enrich → score. The file exports a function that receives `model` and `tools` and returns a compiled graph.

```typescript
import { StateGraph, Annotation, MessagesAnnotation } from "@langchain/langgraph";
import { ToolNode } from "@langchain/langgraph/prebuilt";
import type { BaseChatModel } from "@langchain/core/language_models/chat_models";
import type { StructuredToolInterface } from "@langchain/core/tools";

const ProspectorState = Annotation.Root({
    ...MessagesAnnotation.spec,
    enriched: Annotation<boolean>({ default: () => false }),
});

export default function buildGraph(model: BaseChatModel, tools: StructuredToolInterface[]) {
    const modelWithTools = model.bindTools(tools);
    const toolNode = new ToolNode(tools);

    async function research(state: typeof ProspectorState.State) {
        const response = await modelWithTools.invoke([
            ...state.messages,
            { role: "system", content: "Research this lead using web_search and web_fetch. Do NOT update any records yet." },
        ]);
        return { messages: [response] };
    }

    async function enrich(state: typeof ProspectorState.State) {
        const response = await modelWithTools.invoke([
            ...state.messages,
            { role: "system", content: "Save your findings: create a lead record and research_notes using mutate_data." },
        ]);
        return { messages: [response], enriched: true };
    }

    async function score(state: typeof ProspectorState.State) {
        const response = await model.invoke([
            ...state.messages,
            { role: "system", content: "Score this lead 1-10 based on your research. Update the lead's score field and summarize." },
        ]);
        return { messages: [response] };
    }

    function routeFromResearch(state: typeof ProspectorState.State) {
        const last = state.messages[state.messages.length - 1];
        return last.tool_calls?.length ? "tools" : "enrich";
    }

    function routeFromEnrich(state: typeof ProspectorState.State) {
        const last = state.messages[state.messages.length - 1];
        return last.tool_calls?.length ? "tools" : "score";
    }

    function routeFromTools(state: typeof ProspectorState.State) {
        return state.enriched ? "enrich" : "research";
    }

    return new StateGraph(ProspectorState)
        .addNode("research", research)
        .addNode("enrich", enrich)
        .addNode("score", score)
        .addNode("tools", toolNode)
        .addEdge("__start__", "research")
        .addConditionalEdges("research", routeFromResearch)
        .addConditionalEdges("enrich", routeFromEnrich)
        .addConditionalEdges("tools", routeFromTools)
        .addEdge("score", "__end__")
        .compile();
}
```

### Tool implementations

Tools call Core CRUD via `fetch(${ROOTCX_RUNTIME_URL}/api/v1/apps/${appId}/collections/${entity})` with an `X-Agent-Id` header. Core authenticates the agent and applies RBAC. Full tool code: see `query-data.ts`, `mutate-data.ts`, `web-search.ts`, `web-fetch.ts` in the runtime package — each is a `@langchain/core/tools` `tool()` with Zod schema.

#### `registry.ts`

```typescript
import type { StructuredToolInterface } from "@langchain/core/tools";
import { createQueryDataTool } from "./query-data";
import { createMutateDataTool } from "./mutate-data";
import { createWebSearchTool } from "./web-search";
import { createWebFetchTool } from "./web-fetch";

interface ToolContext {
    appId: string;
    agentId: string;
    runtimeUrl: string;
}

const TOOL_FACTORIES: Record<string, (ctx: ToolContext) => StructuredToolInterface> = {
    query_data:  (ctx) => createQueryDataTool(ctx.appId, ctx.agentId, ctx.runtimeUrl),
    mutate_data: (ctx) => createMutateDataTool(ctx.appId, ctx.agentId, ctx.runtimeUrl),
    web_search:  ()    => createWebSearchTool(),
    web_fetch:   ()    => createWebFetchTool(),
};

export function buildToolRegistry(enabledTools: string[], ctx: ToolContext): StructuredToolInterface[] {
    return enabledTools
        .filter((name) => name in TOOL_FACTORIES)
        .map((name) => TOOL_FACTORIES[name](ctx));
}
```

---

## How the agent accesses its own data

```
User: "Research the CTO of Acme Corp"
  │
  ▼
Agent worker (LangGraph in Bun)
  │
  │  web_search({ query: "Acme Corp CTO" })
  │  ──► Brave API → returns search results
  │
  │  web_fetch({ url: "https://linkedin.com/in/jane-doe-acme" })
  │  ──► HTTP GET → returns profile text
  │
  │  mutate_data({ entity: "leads", action: "create", data: { name: "Jane Doe", company: "Acme Corp", title: "CTO", linkedin_url: "...", status: "researched" } })
  │  ──► POST /api/v1/apps/sales-prospector/collections/leads
  │       ├─ Auth: identity = agent:prospector
  │       ├─ RBAC: role has "create" on "leads" → allowed
  │       ├─ Audit: logged
  │       └─ Returns: created record with ID
  │
  │  mutate_data({ entity: "research_notes", action: "create", data: { lead_id: "...", source: "LinkedIn", content: "CTO since 2023, prev VP Eng at...", category: "person_info" } })
  │  ──► POST /api/v1/apps/sales-prospector/collections/research_notes
  │       ├─ RBAC: role has "create" on "research_notes" → allowed
  │       ├─ Audit: logged
  │       └─ Returns: created record
  │
  │  LLM summarizes: "Found Jane Doe, CTO at Acme Corp since 2023..."
  │
  ▼
Streaming response via SSE
```

The agent creates its own data. Every mutation is audit-logged with `agent:prospector` as the actor. The agent can't delete leads — no policy grants it, RBAC denies it.

---

## Memory

**Session memory** (within a conversation): When `memory.enabled: true`, Core loads previous messages from `agent_sessions.messages` and passes them as `history`. Multi-turn works automatically.

**Long-term memory** (across sessions): The agent's own `dataContract` IS its memory. The sales prospector stores research in `leads` and `research_notes`. On the next session, it queries its own tables:

```
"What do we know about Acme Corp?"
  → query_data({ entity: "leads", filter: { company: "Acme Corp" } })
  → query_data({ entity: "research_notes", filter: { lead_id: "..." } })
  → Agent synthesizes from its own stored research
```

No vector database. No embeddings. The agent's data IS its knowledge base.

---

## What gets built (new code only)

### Rust (Core side)

**1. Extend `AppManifest`** in `crates/shared-types/src/lib.rs`
Add `agents: Option<HashMap<String, AgentDefinition>>` with sub-structs.

**2. `AgentExtension`** in `core/src/extensions/agents/`
New `RuntimeExtension`:
- `bootstrap()`: create `rootcx_system.agents` + `rootcx_system.agent_sessions` tables
- `on_app_installed()`: read `manifest.agents`, upsert into `rootcx_system.agents`, auto-create `agent:{id}` role, translate `access` entries into RBAC policies, extract `tool:` entries as tool allowlist
- `routes()`: invoke (SSE), list agents, list sessions, session detail

**3. IPC messages** in `core/src/ipc.rs`
Add `AgentInvoke` (outbound) and `AgentChunk`/`AgentDone`/`AgentError` (inbound).

**4. Deploy extension** in `core/src/routes/deploy.rs`
If `manifest.agents` is non-empty: read system prompt files, start agent worker via existing `WorkerManager`.

**5. Scaffold layer** in `studio/src-tauri/src/scaffold/layers/agent.rs`
Generate manifest + agent folder + `system.md` + `package.json`.

### TypeScript (Agent side)

**6. `@rootcx/agent-runtime`** in `runtime/agent/`
Entry point, runner, default graph, provider, IPC, and tool implementations as detailed above.

### Forge context

**7. Agent skill** in `.agents/skills/agent-development/SKILL.md`
Teaches Forge: manifest schema, RBAC patterns, tool catalog, system prompt best practices, LangGraph patterns.

---

## The demo

```
1. Open Studio
2. Open Forge: "Build me a sales prospector agent"
3. Forge generates manifest + system.md + graph.ts
4. Click Deploy

5. Invoke:
   POST /api/v1/apps/sales-prospector/agents/prospector/invoke
   { "message": "Research Jane Doe, CTO at Acme Corp" }

6. Watch (SSE streaming):
   → Searches "Jane Doe CTO Acme Corp"
   → Fetches LinkedIn profile
   → Searches "Acme Corp news funding"
   → Creates lead record: Jane Doe, CTO, Acme Corp
   → Creates research notes: company info, person info
   → Scores lead: 8/10 — "Strong fit, Series B, expanding EU"
   → Returns full research summary

7. Audit log:
   agent:prospector CREATE leads (Jane Doe)
   agent:prospector CREATE research_notes (company_info)
   agent:prospector CREATE research_notes (person_info)
   agent:prospector UPDATE leads (score: 8, status: qualified)

8. Next session:
   "Show me all qualified leads"
   → query_data({ entity: "leads", filter: { status: "qualified" } })
   → Returns Jane Doe and any other leads the agent has researched
   → Responds with formatted summary

9. Cross-app read:
   "Check if Jane Doe is already in our CRM"
   → query_data({ app: "crm", entity: "contacts", filter: { email: "jane@acme.com" } })
   → Core checks policy: agent:prospector has read on app:crm/contacts → allowed
   → Returns matching CRM contact (or empty)

10. Unauthorized action:
    Agent tries to delete a lead → RBAC denies (no delete policy)
    → Clear refusal, logged in audit
```

---

## Definition of done

- [ ] `manifest.json` with `agents` section accepted by `install_app()`
- [ ] Installation auto-creates `agent:{id}` role from `access` list
- [ ] Agents registered in `rootcx_system.agents` on install
- [ ] Re-install updates agent config without losing sessions
- [ ] `POST /invoke` returns SSE streaming response
- [ ] Tools loaded from `tool:` access entries — no `tool:web_search` entry = no web search tool
- [ ] `query_data` calls Core CRUD with agent identity → RBAC enforced
- [ ] `mutate_data` calls Core CRUD → mutations appear in audit log
- [ ] Cross-app read works: `app:crm/contacts` access entry → agent can query CRM contacts
- [ ] Agent with insufficient permissions gets clear refusal (not a crash)
- [ ] `web_search` returns real results (Brave or Tavily)
- [ ] `web_fetch` extracts readable content from URLs
- [ ] Default ReAct graph works out of the box (no `graph.ts` needed)
- [ ] Custom `graph.ts` overrides default graph when present
- [ ] Session messages persist across turns when `memory.enabled: true`
- [ ] Session resumes with previous messages on new invoke with same session
- [ ] Agent persists data to its own collections via `mutate_data`
- [ ] `maxTurns` stops the agent loop
- [ ] Agent worker restarts on crash (existing supervisor)
- [ ] Forge generates working agent from natural language description
- [ ] `rootcx new --template agent` generates working starter project
- [ ] Full demo scenario runs end-to-end

---

## What comes next (future iterations)

| Capability | What it adds |
|-----------|-------------|
| Semantic memory (pgvector) | Vector search over agent data instead of keyword match |
| Scheduled agents (cron) | Agents run on a schedule without human trigger |
| Email / HTTP tools | Agent sends emails, calls external APIs |
| Channel adapters | Agent responds in Slack, Discord, WhatsApp, email |
| Webhooks | External services trigger agents |
| Fleet Dashboard | Studio UI to monitor all agents, costs, errors |
| Chat widget | Embeddable chat component in web apps |
| Budget limits (`maxBudgetUsd`) | Track token costs per invocation, abort when ceiling reached |
| Multi-agent delegation | Agents invoke other agents |
| MCP client | Connect to external MCP tool servers |
| Approval flows | Human-in-the-loop before destructive actions |
| Domain scoping | Restrict `tool:web_fetch` to specific domains (e.g. only linkedin.com) via policy `scope` field |
| Browser automation | Agent navigates web pages via headless Chromium |

Each is a natural extension. None requires rearchitecting.
