use std::path::PathBuf;
use std::sync::Arc;

use rootcx_forge::{ForgeConfig, ForgeEngine};
use rootcx_client::RuntimeClient;
use serde_json::Value;
use tauri::{AppHandle, Emitter, State};
use tracing::{info, warn};
use uuid::Uuid;

pub type ForgeState = Arc<tokio::sync::OnceCell<ForgeEngine>>;

pub fn new_state() -> ForgeState {
    Arc::new(tokio::sync::OnceCell::new())
}

fn db_path() -> PathBuf {
    rootcx_platform::dirs::rootcx_home()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("forge.db")
}

pub async fn init(state: &ForgeState, client: RuntimeClient) {
    ensure_bundled_skills().await;
    match ForgeEngine::new(&db_path()).await {
        Ok(mut e) => {
            e.set_integration_fetcher(Arc::new(move || {
                let c = client.clone();
                Box::pin(async move { c.list_integrations().await.map_err(|e| e.to_string()) })
            }));
            let _ = state.set(e);
            info!("forge: ready");
        }
        Err(e) => warn!("forge: init failed: {e}"),
    }
}

const BUNDLED_SKILLS: &[(&str, &str)] = &[
    ("rootcx/SKILL.md", include_str!("../skills/rootcx/SKILL.md")),
    ("rootcx/rules/agent.md", include_str!("../skills/rootcx/rules/agent.md")),
    ("rootcx/rules/backend-worker.md", include_str!("../skills/rootcx/rules/backend-worker.md")),
    ("rootcx/rules/manifest.md", include_str!("../skills/rootcx/rules/manifest.md")),
    ("rootcx/rules/rest-api.md", include_str!("../skills/rootcx/rules/rest-api.md")),
    ("rootcx/rules/rest-api-collections.md", include_str!("../skills/rootcx/rules/rest-api-collections.md")),
    ("rootcx/rules/rest-api-integrations.md", include_str!("../skills/rootcx/rules/rest-api-integrations.md")),
    ("rootcx/rules/rest-api-jobs.md", include_str!("../skills/rootcx/rules/rest-api-jobs.md")),
    ("rootcx/rules/sdk-hooks.md", include_str!("../skills/rootcx/rules/sdk-hooks.md")),
    ("rootcx/rules/ui.md", include_str!("../skills/rootcx/rules/ui.md")),
    ("rootcx/rules/ui-components.md", include_str!("../skills/rootcx/rules/ui-components.md")),
    ("rootcx/rules/templates/system.md", include_str!("../skills/rootcx/rules/templates/system.md")),
];

async fn ensure_bundled_skills() {
    let dir = match crate::state::skills_dir() {
        Ok(d) => d,
        Err(_) => return,
    };
    for (rel_path, content) in BUNDLED_SKILLS {
        let dest = dir.join(rel_path);
        if let Some(parent) = dest.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        let _ = tokio::fs::write(&dest, content).await;
    }
    cleanup_legacy_instructions().await;
}

async fn cleanup_legacy_instructions() {
    if let Ok(dir) = crate::state::config_dir() {
        let legacy = dir.join("instructions").join("rootcx-sdk.md");
        let _ = tokio::fs::remove_file(legacy).await;
    }
}

async fn build_config(client: &RuntimeClient) -> Result<ForgeConfig, String> {
    let ai = client.get_forge_config().await.map_err(|e| e.to_string())?;
    let env = client.get_platform_env().await.unwrap_or_default();

    let provider_str = ai.get("model").and_then(|m| m.as_str()).unwrap_or("anthropic/claude-sonnet-4-6");
    let (provider, model) = parse_provider_model(provider_str);
    let secret_key = match provider {
        rootcx_types::ProviderType::Anthropic => "ANTHROPIC_API_KEY",
        rootcx_types::ProviderType::OpenAI => "OPENAI_API_KEY",
        rootcx_types::ProviderType::Bedrock => "AWS_BEARER_TOKEN_BEDROCK",
    };
    let api_key = env.get(secret_key).cloned();

    info!("forge: provider={provider:?} model={model} key={}", if api_key.is_some() { "ok" } else { "missing" });

    let region = ai.get("region").and_then(|r| r.as_str()).map(String::from);
    let skills_dirs = crate::state::skills_dirs();
    Ok(ForgeConfig { provider, model, api_key, region, system_prompt: None, skills_dirs })
}

fn parse_provider_model(s: &str) -> (rootcx_types::ProviderType, String) {
    use rootcx_types::ProviderType::*;
    if let Some(m) = s.strip_prefix("openai/") { return (OpenAI, m.into()); }
    if s.starts_with("bedrock/") || s.starts_with("amazon-bedrock/") {
        let model = s.split_once('/').map(|(_, m)| m).unwrap_or(s);
        let model = if model.starts_with("us.") || model.starts_with("eu.") || model.starts_with("global.") {
            model.to_string()
        } else {
            format!("us.{model}")
        };
        return (Bedrock, model);
    }
    (Anthropic, s.strip_prefix("anthropic/").unwrap_or(s).into())
}

fn engine(state: &ForgeState) -> Result<&ForgeEngine, String> {
    state.get().ok_or_else(|| {
        warn!("forge: engine requested but OnceCell is empty (init not completed yet)");
        "forge not ready — try again in a moment".into()
    })
}

fn emit_fn(app: AppHandle) -> rootcx_forge::engine::EmitFn {
    Arc::new(move |event: &str, payload: Value| {
        if let Err(e) = app.emit(event, &payload) {
            tracing::error!("forge emit {event}: {e}");
        }
    })
}

#[tauri::command]
pub async fn forge_set_cwd(state: State<'_, ForgeState>, path: String) -> Result<(), String> {
    crate::commands::validate_fs_path(&path)?;
    engine(&state)?.set_cwd(PathBuf::from(path)).await;
    Ok(())
}

async fn ensure_config(engine: &ForgeEngine, client: &RuntimeClient) {
    if engine.config().await.api_key.is_some() { return; }
    match build_config(client).await {
        Ok(c) => engine.set_config(c).await,
        Err(e) => warn!("forge: config load failed: {e}"),
    }
}

#[tauri::command]
pub async fn forge_reload_config(state: State<'_, ForgeState>, app_state: State<'_, crate::state::AppState>) -> Result<(), String> {
    let e = engine(&state)?;
    let c = build_config(&app_state.client()).await?;
    e.set_config(c).await;
    Ok(())
}

#[tauri::command]
pub async fn forge_create_session(state: State<'_, ForgeState>) -> Result<rootcx_forge::session::Session, String> {
    engine(&state)?.create_session().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn forge_list_sessions(state: State<'_, ForgeState>) -> Result<Vec<rootcx_forge::session::Session>, String> {
    engine(&state)?.list_sessions().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn forge_get_messages(state: State<'_, ForgeState>, session_id: String) -> Result<Vec<rootcx_forge::session::MessageWithParts>, String> {
    engine(&state)?.get_messages(&session_id).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn forge_send_message(app: AppHandle, state: State<'_, ForgeState>, app_state: State<'_, crate::state::AppState>, session_id: String, text: String) -> Result<(), String> {
    let e = engine(&state)?;
    ensure_config(e, &app_state.client()).await;
    e.send_message(session_id, text, emit_fn(app)).await;
    Ok(())
}

#[tauri::command]
pub async fn forge_abort(state: State<'_, ForgeState>, session_id: String) -> Result<(), String> {
    engine(&state)?.abort(&session_id).await;
    Ok(())
}

#[tauri::command]
pub async fn forge_reply_permission(state: State<'_, ForgeState>, id: Uuid, session_id: String, tool: String, response: String) -> Result<(), String> {
    engine(&state)?.reply_permission(id, &session_id, &tool, &response).await;
    Ok(())
}

#[tauri::command]
pub async fn forge_reply_question(state: State<'_, ForgeState>, id: Uuid, answers: Vec<Vec<String>>) -> Result<(), String> {
    engine(&state)?.reply_question(id, answers).await;
    Ok(())
}

#[tauri::command]
pub async fn forge_reject_question(state: State<'_, ForgeState>, id: Uuid) -> Result<(), String> {
    engine(&state)?.reject_question(id).await;
    Ok(())
}
