# RootCX AI Agents

Agents are apps with a `backend/` containing a LangGraph agent. Same manifest, same deploy, same RBAC. Core passes LLM credentials via IPC.

## Project structure

```
my-agent/
├── manifest.json                  # Data contract (no agent field)
├── .rootcx/launch.json
├── src/App.tsx                    # Chat UI
└── backend/
    ├── agent.json                 # Agent config (limits, memory, supervision)
    ├── agent/system.md            # System prompt
    ├── index.ts                   # LangGraph agent + IPC bridge
    └── package.json               # @langchain/langgraph, @langchain/openai, zod
```

## backend/agent.json

Agent config — read by Core at deploy time, separate from manifest:

```json
{
  "name": "<name>",
  "description": "<description>",
  "systemPrompt": "./agent/system.md",
  "memory": { "enabled": true },
  "limits": { "maxTurns": 50, "maxContextTokens": 100000, "keepRecentMessages": 10 },
  "supervision": { "mode": "autonomous" }
}
```

LLM provider selected at scaffold time. Platform secrets (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `AWS_BEARER_TOKEN_BEDROCK`) set via dashboard. Core passes credentials to agent at startup.

See [system prompt template](./templates/system.md) for starting file.

## Tools & permissions

All registered tools available via IPC. RBAC permissions declared in `manifest.json` `permissions.permissions[]` as `{ "key": "<entity>.<action>", "description": "..." }` strings.

## Backend code

The scaffold generates a single `index.ts` — the developer owns the code:
- LangGraph `createReactAgent` handles the ReAct loop and streaming
- Provider-specific LangChain SDK (ChatAnthropic, ChatOpenAI, ChatBedrockConverse)
- IPC bridge (JSON-lines stdin/stdout) connects to Core for tool calls and supervision
- Dependencies: `@langchain/langgraph`, provider package, `@langchain/core`, `zod`

## Invocation

```
POST /api/v1/apps/{app_id}/agent/invoke
{ "message": "...", "session_id": "optional-uuid" }
```

Response: SSE stream (`chunk`, `tool_call_started`, `tool_call_completed`, `approval_required`, `done`, `error` events).

Other endpoints:
- `GET /api/v1/apps/{app_id}/agent` — agent config
- `GET /api/v1/apps/{app_id}/agent/sessions` — list sessions
- `GET /api/v1/apps/{app_id}/agent/sessions/{session_id}` — get session
