pub mod compactor;
pub mod engine;
pub mod error;
pub mod permission;
pub mod provider;
pub mod question;
pub mod schema;
pub mod session;
pub mod tools;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use error::ForgeError;
use provider::ProviderKind;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tokio::sync::{Mutex, RwLock};
use tokio::task::{AbortHandle, JoinHandle};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgeConfig {
    pub provider: ProviderKind,
    pub model: String,
    pub api_key: Option<String>,
    pub region: Option<String>,
    pub system_prompt: Option<String>,
    pub instructions: Vec<String>,
}

impl Default for ForgeConfig {
    fn default() -> Self {
        Self {
            provider: ProviderKind::Anthropic,
            model: "claude-sonnet-4-20250514".into(),
            api_key: None,
            region: None,
            system_prompt: None,
            instructions: vec![],
        }
    }
}

pub struct ForgeEngine {
    pool: PgPool,
    cwd: Arc<RwLock<PathBuf>>,
    config: Arc<RwLock<ForgeConfig>>,
    active_loops: Arc<Mutex<HashMap<Uuid, AbortHandle>>>,
    permissions: Arc<permission::PendingPermissions>,
    questions: Arc<question::PendingQuestions>,
}

impl ForgeEngine {
    pub async fn new(pg_url: &str) -> Result<Self, ForgeError> {
        let pool = PgPool::connect(pg_url).await?;
        schema::bootstrap(&pool).await?;
        tracing::info!("forge engine initialized");

        Ok(Self {
            pool,
            cwd: Arc::new(RwLock::new(
                std::env::var("HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|_| PathBuf::from("/")),
            )),
            config: Arc::new(RwLock::new(ForgeConfig::default())),
            active_loops: Arc::new(Mutex::new(HashMap::new())),
            permissions: permission::PendingPermissions::new(),
            questions: question::PendingQuestions::new(),
        })
    }

    pub async fn config(&self) -> ForgeConfig {
        self.config.read().await.clone()
    }

    pub async fn set_cwd(&self, path: PathBuf) {
        *self.cwd.write().await = path;
    }

    pub async fn set_config(&self, config: ForgeConfig) {
        *self.config.write().await = config;
    }

    pub async fn create_session(&self) -> Result<session::Session, ForgeError> {
        let cwd = self.cwd.read().await.display().to_string();
        session::create_session(&self.pool, &cwd).await
    }

    pub async fn list_sessions(&self) -> Result<Vec<session::Session>, ForgeError> {
        session::list_sessions(&self.pool).await
    }

    pub async fn get_messages(
        &self,
        session_id: Uuid,
    ) -> Result<Vec<session::MessageWithParts>, ForgeError> {
        session::get_messages_with_parts(&self.pool, session_id).await
    }

    pub async fn send_message(
        &self,
        session_id: Uuid,
        text: String,
        emit_fn: engine::EmitFn,
    ) -> JoinHandle<()> {
        let config = self.config.read().await.clone();
        let system_prompt = self.build_system_prompt(&config).await;

        let ctx = engine::LoopContext {
            pool: self.pool.clone(),
            session_id,
            cwd: self.cwd.read().await.clone(),
            system_prompt,
            provider: provider::build_provider(
                &config.provider, &config.model, config.api_key.as_deref(), config.region.as_deref(),
            ),
            compactor: Box::new(compactor::LlmSummarizer),
            config,
            permissions: self.permissions.clone(),
            questions: self.questions.clone(),
            emit: emit_fn,
        };

        let active = self.active_loops.clone();
        let handle = tokio::spawn(async move {
            if let Err(e) = engine::agentic_loop(ctx, &text).await {
                tracing::warn!(error = %e, "agentic loop failed");
            }
            active.lock().await.remove(&session_id);
        });

        self.active_loops
            .lock()
            .await
            .insert(session_id, handle.abort_handle());

        handle
    }

    pub async fn abort(&self, session_id: Uuid) {
        if let Some(handle) = self.active_loops.lock().await.remove(&session_id) {
            handle.abort();
        }
    }

    pub async fn reply_permission(&self, id: Uuid, session_id: Uuid, tool: &str, response: &str) {
        let resp = permission::PermissionResponse::parse(response);
        self.permissions.reply(id, session_id, tool, resp).await;
    }

    pub async fn reply_question(&self, id: Uuid, answers: Vec<Vec<String>>) {
        self.questions.reply(id, answers).await;
    }

    pub async fn reject_question(&self, id: Uuid) {
        self.questions.reject(id).await;
    }

    async fn build_system_prompt(&self, config: &ForgeConfig) -> String {
        let mut prompt = config.system_prompt.clone().unwrap_or_else(|| {
            "You are an expert software engineer. You help users build, debug, and improve code.\n\
             Use the available tools to read, search, and modify files. Always read files before editing.\n\
             Be concise. Focus on what was asked.".into()
        });
        for pattern in &config.instructions {
            for entry in glob::glob(pattern).into_iter().flatten().flatten() {
                if let Ok(content) = tokio::fs::read_to_string(&entry).await {
                    prompt.push_str("\n\n");
                    prompt.push_str(&content);
                }
            }
        }
        prompt
    }
}

