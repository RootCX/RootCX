use crate::emitter::Emitter;
use crate::types::{AnswerValue, Layer, LayerFuture, ScaffoldContext};

const TPL_AGENT_APP: &str = include_str!("../../templates/scaffold/agent-app.tsx");
const TPL_INDEX: &str = include_str!("../../templates/scaffold/agent/index.ts");

struct LlmConfig {
    import: &'static str,
    init: &'static str,
    dep_name: &'static str,
    dep_version: &'static str,
}

fn llm_config(provider: &str) -> LlmConfig {
    match provider {
        "openai" => LlmConfig {
            import: r#"import { ChatOpenAI } from "@langchain/openai";"#,
            init: r#"new ChatOpenAI({ apiKey: credentials.OPENAI_API_KEY, model: "gpt-4.1" })"#,
            dep_name: "@langchain/openai",
            dep_version: "^1.4.0",
        },
        "bedrock" => LlmConfig {
            import: r#"import { ChatBedrockConverse } from "@langchain/aws";"#,
            init: concat!(
                r#"(() => { "#,
                r#"if (credentials.AWS_BEARER_TOKEN_BEDROCK) "#,
                r#"process.env.AWS_BEARER_TOKEN_BEDROCK = credentials.AWS_BEARER_TOKEN_BEDROCK; "#,
                r#"return new ChatBedrockConverse({ "#,
                r#"region: "us-east-1", model: "global.anthropic.claude-sonnet-4-6", "#,
                r#"...(credentials.AWS_ACCESS_KEY_ID "#,
                r#"? { credentials: { accessKeyId: credentials.AWS_ACCESS_KEY_ID, "#,
                r#"secretAccessKey: credentials.AWS_SECRET_ACCESS_KEY ?? "" } } : {}) }); })()"#,
            ),
            dep_name: "@langchain/aws",
            dep_version: "^1.3.0",
        },
        _ => LlmConfig {
            import: r#"import { ChatAnthropic } from "@langchain/anthropic";"#,
            init: r#"new ChatAnthropic({ apiKey: credentials.ANTHROPIC_API_KEY, model: "claude-sonnet-4-6" })"#,
            dep_name: "@langchain/anthropic",
            dep_version: "^1.3.0",
        },
    }
}

pub struct AgentLayer;

impl Layer for AgentLayer {
    fn emit<'a>(&'a self, ctx: &'a ScaffoldContext, e: &'a Emitter) -> LayerFuture<'a> {
        Box::pin(async move {
            let provider = match ctx.answers.get("llm_provider") {
                Some(AnswerValue::Text(v)) => v.as_str(),
                _ => "anthropic",
            };
            let llm = llm_config(provider);

            e.write_json("backend/agent.json", &serde_json::json!({
                "name": ctx.app_id,
                "description": format!("AI agent for {}", ctx.app_id),
                "systemPrompt": "./agent/system.md",
                "memory": { "enabled": true },
                "limits": { "maxTurns": 50, "maxContextTokens": 100000, "keepRecentMessages": 10 },
                "supervision": { "mode": "autonomous" }
            })).await?;

            e.write("backend/agent/system.md", &format!(
                "You are the {} agent.\n\n## Your role\nDescribe what this agent does.\n\n## Tools\nYou have access to tools for querying and mutating data, searching the web, and fetching pages.\nUse them as needed to fulfill user requests.\n",
                ctx.app_id
            )).await?;

            let index = TPL_INDEX
                .replace("__LLM_IMPORT__", llm.import)
                .replace("__LLM_INIT__", llm.init);
            e.write("backend/index.ts", &index).await?;

            e.write_json("backend/package.json", &serde_json::json!({
                "name": format!("{}-backend", ctx.app_id),
                "version": "0.1.0",
                "private": true,
                "type": "module",
                "dependencies": {
                    "langchain": "^1.2.0",
                    "@langchain/core": "^1.1.0",
                    llm.dep_name: llm.dep_version,
                    "zod": "^3.25.0"
                }
            })).await?;

            e.write("src/App.tsx", &TPL_AGENT_APP.replace("__APP_ID__", &ctx.app_id)).await?;

            Ok(())
        })
    }
}
