use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OsStatus {
    pub runtime: RuntimeStatus,
    pub postgres: PostgresStatus,
    pub forge: ForgeStatus,
}

impl OsStatus {
    pub fn offline() -> Self {
        Self {
            runtime: RuntimeStatus { version: String::new(), state: ServiceState::Offline },
            postgres: PostgresStatus { state: ServiceState::Offline, port: None, data_dir: None },
            forge: ForgeStatus { state: ServiceState::Offline, port: None },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgeStatus {
    pub state: ServiceState,
    pub port: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeStatus {
    pub version: String,
    pub state: ServiceState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostgresStatus {
    pub state: ServiceState,
    pub port: Option<u16>,
    pub data_dir: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServiceState {
    Online,
    Offline,
    Starting,
    Stopping,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppManifest {
    pub app_id: String,
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub permissions: Option<PermissionsContract>,
    #[serde(default)]
    pub data_contract: Vec<EntityContract>,
    #[serde(default)]
    pub agent: Option<AgentDefinition>,
}

fn default_version() -> String {
    "0.0.1".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntityContract {
    pub entity_name: String,
    pub fields: Vec<FieldContract>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldContract {
    pub name: String,
    #[serde(rename = "type")]
    pub field_type: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub default_value: Option<JsonValue>,
    #[serde(default)]
    pub enum_values: Option<Vec<String>>,
    #[serde(default)]
    pub references: Option<FieldReference>,
    #[serde(default)]
    pub is_primary_key: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldReference {
    pub entity: String,
    pub field: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledApp {
    pub id: String,
    pub name: String,
    pub version: String,
    pub status: String,
    pub entities: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionsContract {
    #[serde(default)]
    pub permissions: Vec<PermissionDeclaration>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionDeclaration {
    pub key: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaChange {
    pub entity: String,
    pub change_type: String,
    pub column: String,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaVerification {
    pub compliant: bool,
    pub changes: Vec<SchemaChange>,
}

pub const DEFAULT_MODEL: &str = "claude-sonnet-4-6";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderType {
    Anthropic,
    OpenAI,
    Bedrock,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ProviderConfig {
    #[serde(rename = "anthropic")]
    Anthropic {
        model: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        api_key: Option<String>,
    },
    #[serde(rename = "openai")]
    OpenAI {
        model: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        api_key: Option<String>,
    },
    /// Bedrock uses IAM role-based auth server-side; no api_key field needed.
    #[serde(rename = "bedrock")]
    Bedrock {
        model: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        region: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiConfig {
    pub provider: ProviderType,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self { provider: ProviderType::Anthropic, model: DEFAULT_MODEL.into(), region: None }
    }
}

impl AiConfig {
    pub fn forge_model_string(&self) -> String {
        match self.provider {
            ProviderType::Bedrock => format!("amazon-bedrock/anthropic.{}", self.model),
            ProviderType::Anthropic => format!("anthropic/{}", self.model),
            ProviderType::OpenAI => format!("openai/{}", self.model),
        }
    }

    pub fn agent_provider_config(&self) -> ProviderConfig {
        match self.provider {
            ProviderType::Anthropic => ProviderConfig::Anthropic {
                model: self.model.clone(),
                api_key: Some("${ANTHROPIC_API_KEY}".into()),
            },
            ProviderType::OpenAI => ProviderConfig::OpenAI {
                model: self.model.clone(),
                api_key: Some("${OPENAI_API_KEY}".into()),
            },
            ProviderType::Bedrock => ProviderConfig::Bedrock {
                model: format!("us.anthropic.{}", self.model),
                region: self.region.clone(),
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDefinition {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub provider: Option<ProviderConfig>,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub graph: Option<String>,
    #[serde(default)]
    pub memory: Option<AgentMemory>,
    #[serde(default)]
    pub limits: Option<AgentLimits>,
    #[serde(default)]
    pub access: Vec<AgentAccessEntry>,
    #[serde(default)]
    pub supervision: Option<SupervisionConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMemory {
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentLimits {
    #[serde(default)]
    pub max_turns: Option<u32>,
    #[serde(default)]
    pub max_context_tokens: Option<u64>,
    #[serde(default)]
    pub keep_recent_messages: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SupervisionConfig {
    pub mode: SupervisionMode,
    #[serde(default)]
    pub policies: Vec<SupervisionPolicy>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SupervisionMode {
    Autonomous,
    Supervised,
    Strict,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SupervisionPolicy {
    pub action: String,
    #[serde(default)]
    pub entity: Option<String>,
    #[serde(default)]
    pub requires: Option<String>,
    #[serde(default)]
    pub rate_limit: Option<RateLimit>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimit {
    pub max: u32,
    pub window: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentAccessEntry {
    pub entity: String,
    pub actions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDescriptor {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}
