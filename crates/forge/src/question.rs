use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, oneshot};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionOption {
    pub label: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionInfo {
    pub question: String,
    pub header: String,
    pub options: Vec<QuestionOption>,
    #[serde(default)]
    pub multiple: bool,
    #[serde(default)]
    pub custom: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionRequest {
    pub id: Uuid,
    pub session_id: Uuid,
    pub questions: Vec<QuestionInfo>,
}

pub enum QuestionResponse {
    Answered(Vec<Vec<String>>),
    Rejected,
}

#[derive(Default)]
pub struct PendingQuestions {
    pending: Mutex<HashMap<Uuid, oneshot::Sender<QuestionResponse>>>,
}

impl PendingQuestions {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub async fn ask(
        &self,
        session_id: Uuid,
        questions: Vec<QuestionInfo>,
    ) -> (QuestionRequest, oneshot::Receiver<QuestionResponse>) {
        let id = Uuid::new_v4();
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        let req = QuestionRequest {
            id,
            session_id,
            questions,
        };

        (req, rx)
    }

    pub async fn reply(&self, id: Uuid, answers: Vec<Vec<String>>) {
        if let Some(tx) = self.pending.lock().await.remove(&id) {
            let _ = tx.send(QuestionResponse::Answered(answers));
        }
    }

    pub async fn reject(&self, id: Uuid) {
        if let Some(tx) = self.pending.lock().await.remove(&id) {
            let _ = tx.send(QuestionResponse::Rejected);
        }
    }
}
