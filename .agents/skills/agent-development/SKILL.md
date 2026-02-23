# Agent Development Skill

Build AI agents on RootCX. Agents are autonomous AI workers that use the same infrastructure as apps — same manifest, same deploy, same RBAC, same audit trail.

**CRITICAL**: Agent projects have **NO frontend**. No React, no JSX/TSX UI components, no Tauri, no Vite, no `src/` folder, no `index.html`. An agent project contains ONLY `manifest.json`, `agents/` directory, and `package.json`.

## Manifest Schema

An agent project has a `manifest.json` with an `agents` section:

```json
{
    "appId": "my-agent",
    "name": "My Agent",
    "version": "0.1.0",
    "description": "What this agent does",
    "dataContract": [
        {
            "entityName": "records",
            "fields": [
                { "name": "title", "type": "text", "required": true },
                { "name": "status", "type": "text", "enumValues": ["new", "done"] }
            ]
        }
    ],
    "agents": {
        "worker": {
            "name": "Worker Agent",
            "description": "What this agent does",
            "model": "claude-sonnet-4-20250514",
            "systemPrompt": "./agents/worker/system.md",
            "graph": "./agents/worker/graph.ts",
            "memory": { "enabled": true },
            "limits": { "maxTurns": 20 },
            "access": [
                { "entity": "records", "actions": ["read", "create", "update"] },
                { "entity": "tool:web_search", "actions": ["use"] },
                { "entity": "tool:web_fetch", "actions": ["use"] }
            ]
        }
    }
}
```

## Access Control

The `access` array defines **everything** the agent can do:

| Pattern | Meaning |
|---------|---------|
| `{ "entity": "records", "actions": ["read", "create"] }` | Read and create records |
| `{ "entity": "tool:web_search", "actions": ["use"] }` | Can search the web |
| `{ "entity": "tool:web_fetch", "actions": ["use"] }` | Can fetch URLs |
| `{ "entity": "app:crm/contacts", "actions": ["read"] }` | Can read CRM contacts (cross-app) |

What's NOT listed = what the agent CANNOT do. No `delete` action = agent can never delete.

Core auto-creates the RBAC role `agent:{id}` from this list. Same enforcement as human roles.

## Available Tools

| Access entry | Tool name | Description |
|-------------|-----------|-------------|
| `tool:web_search` | `web_search` | Search via Brave Search or Tavily |
| `tool:web_fetch` | `web_fetch` | Fetch URL, extract readable content |
| _(any data access)_ | `query_data` | Read from collections (auto-loaded) |
| _(any data access)_ | `mutate_data` | Create/update/delete records (auto-loaded) |

## Project Structure

```
my-agent/
├── manifest.json
├── agents/
│   └── worker/
│       ├── system.md          # System prompt
│       └── graph.ts           # Optional custom LangGraph
└── package.json               # includes @rootcx/agent-runtime
```

## System Prompt Best Practices

Write the system prompt in `agents/{id}/system.md`:

1. **Be specific**: Describe the agent's role, goals, and constraints
2. **Reference data**: Mention the exact entity names the agent works with
3. **Define workflow**: Step-by-step instructions for the agent's task
4. **Set boundaries**: What the agent should NOT do

Example:
```markdown
You are a sales prospector agent. Your job is to research leads and score them.

## Data you work with
- **leads**: Company contacts with name, company, title, score, status
- **research_notes**: Notes from your research, linked to leads

## Workflow
1. When given a name/company, use web_search to find information
2. Use web_fetch to read relevant pages (LinkedIn, company websites)
3. Create a lead record with mutate_data
4. Create research_notes for each finding
5. Score the lead 1-10 and update the lead record

## Rules
- Never delete records
- Always cite your sources in research_notes
- Score conservatively — only 8+ for strong matches
```

## Custom LangGraph

If `graph` is omitted, the agent uses the built-in ReAct loop (call LLM → use tools → repeat).

For custom workflows, export a function from `graph.ts`:

```typescript
import { StateGraph, MessagesAnnotation } from "@langchain/langgraph";
import { ToolNode } from "@langchain/langgraph/prebuilt";
import type { BaseChatModel } from "@langchain/core/language_models/chat_models";
import type { StructuredToolInterface } from "@langchain/core/tools";

export default function buildGraph(
    model: BaseChatModel,
    tools: StructuredToolInterface[],
) {
    const modelWithTools = model.bindTools(tools);
    const toolNode = new ToolNode(tools);

    // Define your nodes and edges here
    // Return a compiled StateGraph
}
```

## Supported Models

Set `model` in the agent definition:
- `claude-sonnet-4-20250514` (default)
- `claude-opus-4-20250514`
- `gpt-4o`, `gpt-4o-mini`

API keys are injected as env vars via the secrets system:
- `ANTHROPIC_API_KEY` for Claude models
- `OPENAI_API_KEY` for GPT models
- `BRAVE_API_KEY` or `TAVILY_API_KEY` for web search

## Memory

When `memory.enabled: true`:
- Session messages persist across turns
- Resume a session by passing `session_id` in the invoke request
- The agent's `dataContract` is its long-term memory (query its own tables)

## Invoking an Agent

```
POST /api/v1/apps/{appId}/agents/{agentId}/invoke
Content-Type: application/json

{ "message": "Research Jane Doe at Acme Corp", "session_id": "optional-uuid" }
```

Response: Server-Sent Events stream with `chunk`, `done`, and `error` events.
