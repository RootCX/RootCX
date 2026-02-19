use crate::scaffold::layers::*;
use crate::scaffold::types::*;
use std::collections::HashMap;

pub struct BlankPreset;

impl Preset for BlankPreset {
    fn info(&self) -> PresetInfo {
        PresetInfo {
            id: "blank",
            name: "Blank",
            description: "Empty project with optional auth, permissions, and backend worker",
            icon: "📦",
        }
    }

    fn questions(&self) -> Vec<Question> {
        vec![
            Question {
                key: "auth".into(),
                label: "Include authentication?".into(),
                question_type: QuestionType::Bool,
                default: Some(AnswerValue::Bool(true)),
                depends_on: None,
            },
            Question {
                key: "permissions".into(),
                label: "Include role-based permissions?".into(),
                question_type: QuestionType::Bool,
                default: Some(AnswerValue::Bool(false)),
                depends_on: Some(DependsOn {
                    key: "auth".into(),
                    equals: AnswerValue::Bool(true),
                }),
            },
            Question {
                key: "backend".into(),
                label: "Include backend worker?".into(),
                question_type: QuestionType::Bool,
                default: Some(AnswerValue::Bool(true)),
                depends_on: None,
            },
        ]
    }

    fn layers(&self, answers: &HashMap<String, AnswerValue>) -> Vec<Box<dyn Layer>> {
        let auth = matches!(answers.get("auth"), Some(AnswerValue::Bool(true)) | None);
        let backend = matches!(answers.get("backend"), Some(AnswerValue::Bool(true)) | None);

        let mut layers: Vec<Box<dyn Layer>> = vec![
            Box::new(CoreLayer),
            Box::new(TauriLayer),
            Box::new(AuthLayer { include_auth: auth }),
        ];
        if backend {
            layers.push(Box::new(BackendLayer));
        }
        layers
    }
}
