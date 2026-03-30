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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AppType {
    #[default]
    App,
    Integration,
    Agent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionDefinition {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub input_schema: Option<JsonValue>,
    #[serde(default)]
    pub output_schema: Option<JsonValue>,
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
    #[serde(default, rename = "type")]
    pub app_type: AppType,
    #[serde(default)]
    pub permissions: Option<PermissionsContract>,
    #[serde(default)]
    pub data_contract: Vec<EntityContract>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<ActionDefinition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_schema: Option<JsonValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_auth: Option<JsonValue>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub webhooks: Vec<String>,
    /// Free-form usage instructions surfaced to AI via list_integrations tool
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    /// Trigger: auto-invoke this agent on entity events
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<TriggerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TriggerConfig {
    pub app_id: String,
    pub entity: String,
    pub on: Vec<String>,
}

fn default_version() -> String {
    "0.0.1".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntityContract {
    pub entity_name: String,
    pub fields: Vec<FieldContract>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_key: Option<String>,
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
#[serde(rename_all = "camelCase")]
pub struct InstalledApp {
    pub id: String,
    pub name: String,
    pub version: String,
    pub status: String,
    #[serde(rename = "type", default)]
    pub app_type: AppType,
    pub entities: Vec<String>,
    #[serde(default)]
    pub has_frontend: bool,
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
    #[serde(default)]
    pub safe: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaVerification {
    pub compliant: bool,
    pub changes: Vec<SchemaChange>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderType {
    Anthropic,
    OpenAI,
    Bedrock,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDefinition {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub memory: Option<AgentMemory>,
    #[serde(default)]
    pub limits: Option<AgentLimits>,
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
#[serde(rename_all = "camelCase")]
pub struct McpServerConfig {
    pub name: String,
    pub transport: McpTransport,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum McpTransport {
    Stdio { command: String, #[serde(default)] args: Vec<String> },
    Http { url: String, #[serde(default)] headers: std::collections::HashMap<String, String> },
    #[deprecated = "use Http"]
    Sse { url: String, #[serde(default)] headers: std::collections::HashMap<String, String> },
    Cli { install: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDescriptor {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text { text: String },
    ToolUse { id: String, name: String, input: serde_json::Value },
    ToolResult { tool_use_id: String, content: String, #[serde(default)] is_error: bool },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

