use std::collections::HashMap;

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
    pub roles: HashMap<String, RoleDefinition>,
    #[serde(default)]
    pub default_role: Option<String>,
    pub policies: Vec<PolicyRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleDefinition {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub inherits: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyRule {
    pub role: String,
    pub entity: String,
    pub actions: Vec<String>,
    #[serde(default)]
    pub ownership: bool,
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
