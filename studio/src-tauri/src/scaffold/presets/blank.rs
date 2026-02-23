use crate::scaffold::layers::*;
use crate::scaffold::types::*;
use std::collections::HashMap;

pub struct BlankPreset;

impl Preset for BlankPreset {
    fn info(&self) -> PresetInfo {
        PresetInfo {
            id: "blank",
            name: "Blank",
            description: "Empty project: app with UI, or AI agent",
            icon: "📦",
        }
    }

    fn questions(&self) -> Vec<Question> {
        vec![
            Question {
                key: "project_type".into(),
                label: "What are you building?".into(),
                question_type: QuestionType::Choice {
                    options: vec![
                        ChoiceOption { value: "app".into(), label: "App (UI + backend)".into() },
                        ChoiceOption { value: "agent".into(), label: "AI Agent (no UI)".into() },
                    ],
                },
                default: Some(AnswerValue::Text("app".into())),
                depends_on: None,
            },
            // ── App-specific questions ──
            Question {
                key: "auth".into(),
                label: "Include authentication?".into(),
                question_type: QuestionType::Bool,
                default: Some(AnswerValue::Bool(true)),
                depends_on: Some(DependsOn { key: "project_type".into(), equals: AnswerValue::Text("app".into()) }),
            },
            Question {
                key: "permissions".into(),
                label: "Include role-based permissions?".into(),
                question_type: QuestionType::Bool,
                default: Some(AnswerValue::Bool(false)),
                depends_on: Some(DependsOn { key: "auth".into(), equals: AnswerValue::Bool(true) }),
            },
            Question {
                key: "backend".into(),
                label: "Include backend worker?".into(),
                question_type: QuestionType::Bool,
                default: Some(AnswerValue::Bool(true)),
                depends_on: Some(DependsOn { key: "project_type".into(), equals: AnswerValue::Text("app".into()) }),
            },
            // ── Agent-specific questions ──
            Question {
                key: "agent_id".into(),
                label: "Agent identifier (lowercase, e.g. prospector)".into(),
                question_type: QuestionType::Text,
                default: Some(AnswerValue::Text("assistant".into())),
                depends_on: Some(DependsOn { key: "project_type".into(), equals: AnswerValue::Text("agent".into()) }),
            },
        ]
    }

    fn layers(&self, answers: &HashMap<String, AnswerValue>) -> Vec<Box<dyn Layer>> {
        let is_agent = matches!(answers.get("project_type"), Some(AnswerValue::Text(v)) if v == "agent");

        if is_agent {
            let agent_id = match answers.get("agent_id") {
                Some(AnswerValue::Text(v)) if !v.is_empty() => v.clone(),
                _ => "assistant".into(),
            };
            vec![Box::new(AgentLayer { agent_id })]
        } else {
            let auth = matches!(answers.get("auth"), Some(AnswerValue::Bool(true)) | None);
            let backend = matches!(answers.get("backend"), Some(AnswerValue::Bool(true)) | None);

            let mut layers: Vec<Box<dyn Layer>> =
                vec![Box::new(CoreLayer), Box::new(TauriLayer), Box::new(AuthLayer { include_auth: auth })];
            if backend {
                layers.push(Box::new(BackendLayer));
            }
            layers
        }
    }
}
