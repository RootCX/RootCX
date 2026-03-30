use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use tokio::sync::{Mutex, oneshot};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRequest {
    pub approval_id: String,
    pub app_id: String,
    pub session_id: String,
    pub invoke_id: String,
    pub call_id: String,
    pub tool_name: String,
    pub args: JsonValue,
    pub reason: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalAction {
    Approve,
    Reject,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApprovalReply {
    pub action: ApprovalAction,
    #[serde(default)]
    pub reason: Option<String>,
}

pub enum ApprovalResponse {
    Approved,
    Rejected { reason: String },
}

struct PendingEntry {
    request: ApprovalRequest,
    tx: oneshot::Sender<ApprovalResponse>,
}

#[derive(Default, Clone)]
pub struct PendingApprovals {
    pending: Arc<Mutex<HashMap<String, PendingEntry>>>,
}

impl PendingApprovals {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn request(
        &self,
        request: ApprovalRequest,
    ) -> oneshot::Receiver<ApprovalResponse> {
        let (tx, rx) = oneshot::channel();
        let id = request.approval_id.clone();
        self.pending.lock().await.insert(id, PendingEntry { request, tx });
        rx
    }

    pub async fn reply(&self, approval_id: &str, response: ApprovalResponse) -> bool {
        if let Some(entry) = self.pending.lock().await.remove(approval_id) {
            let _ = entry.tx.send(response);
            true
        } else {
            false
        }
    }

    pub async fn belongs_to_app(&self, approval_id: &str, app_id: &str) -> bool {
        self.pending.lock().await.get(approval_id)
            .is_some_and(|e| e.request.app_id == app_id)
    }

    pub async fn list(&self, app_id: &str) -> Vec<ApprovalRequest> {
        self.pending.lock().await.values()
            .filter(|e| e.request.app_id == app_id)
            .map(|e| e.request.clone())
            .collect()
    }
}
