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
    pub webhooks: Vec<WebhookDefinition>,
    /// Free-form usage instructions surfaced to AI via list_integrations tool
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    /// Trigger: auto-invoke this agent on entity events
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<TriggerConfig>,
    /// Declarative cron schedules synced on deploy
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub crons: Vec<CronDefinition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Public-access surface. Routes listed here bypass Identity. RPCs that
    /// declare `scope` additionally require a share token whose context
    /// matches the request body on the listed keys.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public: Option<PublicSurface>,
}

/// Declarative public-access surface for an app.
///
/// Anything listed here is reachable without an Authorization header.
/// Anything **not** listed retains the default JWT-required behavior.
///
/// See `core/src/extensions/sharing/` for the runtime enforcement.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicSurface {
    /// Custom RPCs exposed publicly. If `scope` is non-empty, the request
    /// must carry a share token whose `context` matches the request body on
    /// every listed key.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rpcs: Vec<PublicRpc>,
    /// CRUD collections exposed publicly with the listed actions.
    /// Allowed actions: "list", "read", "create", "update", "delete".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub collections: Vec<PublicCollection>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicRpc {
    pub name: String,
    /// Keys to enforce-match between the share token's `context` and the
    /// request body. Empty `scope` means anonymous access (no share token
    /// required). Non-empty means a share token IS required and the listed
    /// keys MUST match exactly.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scope: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicCollection {
    pub entity: String,
    /// Subset of CRUD actions exposed: "list", "read", "create", "update", "delete".
    pub actions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CronDefinition {
    pub name: String,
    pub schedule: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<JsonValue>,
    #[serde(default = "default_overlap_policy")]
    pub overlap_policy: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WebhookDefinition {
    Simple(String),
    #[serde(rename_all = "camelCase")]
    Full { name: String, #[serde(default = "default_post")] method: String },
}

fn default_post() -> String { "POST".into() }

impl WebhookDefinition {
    pub fn name(&self) -> &str {
        match self {
            Self::Simple(s) => s.as_str(),
            Self::Full { name, .. } => name.as_str(),
        }
    }
    pub fn method(&self) -> &str {
        match self {
            Self::Simple(_) => "POST",
            Self::Full { method, .. } => method.as_str(),
        }
    }
}

fn default_overlap_policy() -> String { "skip".into() }

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
    /// Declarative secondary indexes (reconciled by name against pg_indexes at
    /// deploy). Covers the Prisma/Drizzle index surface: composite, unique,
    /// partial, functional, method, operator class, sort/nulls order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub indexes: Vec<IndexContract>,
}

/// A declarative index. Mirrors what Prisma/Drizzle let you declare for
/// PostgreSQL. `where`/`expr`/`ops` are SQL fragments (bounded, not full
/// statements); `name` is auto-derived when omitted.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexContract {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub columns: Vec<IndexColumn>,
    #[serde(default)]
    pub unique: bool,
    /// Index method: btree (default) | hash | gist | gin | spgist | brin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub using: Option<String>,
    /// Partial-index predicate (the `WHERE ...` clause).
    #[serde(rename = "where", default, skip_serializing_if = "Option::is_none")]
    pub where_clause: Option<String>,
    /// Storage parameters, e.g. {"fillfactor":"70"}.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub with: std::collections::BTreeMap<String, String>,
}

/// An index element: either a bare column name, or a spec carrying a column OR
/// expression plus sort/nulls/operator-class.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum IndexColumn {
    Name(String),
    Spec(IndexColumnSpec),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexColumnSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<String>,
    /// Functional-index expression (mutually exclusive with `column`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expr: Option<String>,
    /// asc | desc.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort: Option<String>,
    /// first | last.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nulls: Option<String>,
    /// Operator class, e.g. gin_trgm_ops.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ops: Option<String>,
}

// NOTE: no `rename_all = "camelCase"` here — at the FIELD level the manifest
// format is snake_case (`enum_values`, `on_delete`, …). camelCase silently
// dropped those keys at deser (→ null), so no enum CHECK constraints or FK
// delete rules were ever generated. Snake_case mirrors the real format.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_delete: Option<OnDeletePolicy>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnDeletePolicy {
    Cascade,
    Restrict,
    SetNull,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
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


#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn index_column_deserializes_bare_string_and_spec() {
        let json = r#"{"entityName":"program","fields":[],"indexes":[{"name":"test","columns":["slug"],"unique":true}]}"#;
        let e: EntityContract = serde_json::from_str(json).expect("deser failed");
        assert_eq!(e.indexes.len(), 1, "indexes should parse");
        assert_eq!(e.indexes[0].columns.len(), 1, "columns should parse");
        match &e.indexes[0].columns[0] {
            IndexColumn::Name(n) => assert_eq!(n, "slug"),
            other => panic!("expected Name('slug'), got {:?}", other),
        }
    }

    // Regression for the snake_case field keys (see FieldContract): they must
    // survive deser AND a serialize round-trip, else the stored manifest loses
    // them again.
    #[test]
    fn field_contract_preserves_snake_case_keys() {
        let json = r#"{"name":"status","type":"text","enum_values":["draft","active"],"on_delete":"set_null"}"#;
        let f: FieldContract = serde_json::from_str(json).expect("deser failed");
        assert_eq!(f.enum_values.as_deref(), Some(&["draft".to_string(), "active".to_string()][..]));
        assert_eq!(f.on_delete, Some(OnDeletePolicy::SetNull));

        // Round-trip: re-serialize then deser — values must not be lost (proves
        // serialization is snake_case too, so the stored manifest stays faithful).
        let round: FieldContract =
            serde_json::from_value(serde_json::to_value(&f).unwrap()).expect("round-trip failed");
        assert_eq!(round.enum_values, f.enum_values, "enum_values lost on round-trip");
        assert_eq!(round.on_delete, f.on_delete, "on_delete lost on round-trip");
    }
}
