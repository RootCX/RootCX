use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

/// Status of the RootCX operating system, exposed to frontends.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OsStatus {
    pub kernel: KernelStatus,
    pub postgres: PostgresStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KernelStatus {
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

impl std::fmt::Display for ServiceState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Online => write!(f, "online"),
            Self::Offline => write!(f, "offline"),
            Self::Starting => write!(f, "starting"),
            Self::Stopping => write!(f, "stopping"),
            Self::Error => write!(f, "error"),
        }
    }
}

// ── App Manifest Types ──────────────────────────────────────────────

/// Root structure of an app's manifest.json.
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
    pub routes: Vec<JsonValue>,
    #[serde(default)]
    pub permissions: Vec<JsonValue>,
    #[serde(default)]
    pub data_contract: Vec<EntityContract>,
}

fn default_version() -> String {
    "0.0.1".to_string()
}

/// One entity within an app's dataContract (e.g. "deal", "quote").
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntityContract {
    pub entity_name: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub record_label_field: Option<String>,
    #[serde(default)]
    pub core_entity: Option<String>,
    #[serde(default)]
    pub unique_fields: Option<Vec<String>>,
    pub fields: Vec<FieldContract>,
}

/// A single field definition within an entity.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldContract {
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(rename = "type")]
    pub field_type: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub default_value: Option<JsonValue>,
    #[serde(default)]
    pub validation: Option<FieldValidation>,
    #[serde(default)]
    pub references: Option<FieldReference>,
    #[serde(default)]
    pub is_primary_key: Option<bool>,
    #[serde(default)]
    pub relationship_type: Option<String>,
}

/// Validation rules for a field (enum constraints, min/max, pattern).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldValidation {
    #[serde(rename = "enum", default)]
    pub enum_values: Option<Vec<String>>,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    #[serde(default)]
    pub min_length: Option<usize>,
    #[serde(default)]
    pub max_length: Option<usize>,
    #[serde(default)]
    pub pattern: Option<String>,
}

/// Foreign key reference for entity_link fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldReference {
    pub entity: String,
    pub field: String,
}

/// Summary of an installed app (returned by list_apps).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledApp {
    pub id: String,
    pub name: String,
    pub version: String,
    pub status: String,
    pub entities: Vec<String>,
}
