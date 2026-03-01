use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

/// Boxed async return type for Layer::emit — eliminates Future/Pin imports from every layer file.
pub type LayerFuture<'a> = Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresetInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub icon: &'static str,
}

/// Only show this question when a prior answer matches.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DependsOn {
    pub key: String,
    pub equals: AnswerValue,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Question {
    pub key: String,
    pub label: String,
    pub question_type: QuestionType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<AnswerValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub depends_on: Option<DependsOn>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QuestionType {
    Bool,
    Text,
    Choice { options: Vec<ChoiceOption> },
    EntityList,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChoiceOption {
    pub value: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AnswerValue {
    Bool(bool),
    Text(String),
    Entities(Vec<EntityDef>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityDef {
    pub name: String,
    pub fields: Vec<EntityFieldDef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityFieldDef {
    pub name: String,
    pub field_type: String,
    #[serde(default)]
    pub required: bool,
}

pub struct ScaffoldContext {
    pub app_id: String,
    pub identifier: String,
    pub port: u16,
    pub answers: HashMap<String, AnswerValue>,
    pub ai_config: Option<rootcx_types::AiConfig>,
}

pub trait Preset: Send + Sync {
    fn info(&self) -> PresetInfo;
    fn questions(&self) -> Vec<Question>;
    fn layers(&self, answers: &HashMap<String, AnswerValue>) -> Vec<Box<dyn Layer>>;
}

pub trait Layer: Send + Sync {
    fn emit<'a>(&'a self, ctx: &'a ScaffoldContext, emitter: &'a super::emitter::Emitter) -> LayerFuture<'a>;
}
