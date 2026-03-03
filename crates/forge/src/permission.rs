use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, oneshot};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Permission {
    pub id: Uuid,
    pub session_id: Uuid,
    pub tool: String,
    pub title: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PermissionResponse {
    Once,
    Always,
    Reject,
}

impl PermissionResponse {
    pub fn parse(s: &str) -> Self {
        match s {
            "always" => Self::Always,
            "reject" => Self::Reject,
            _ => Self::Once,
        }
    }
}

#[derive(Default)]
pub struct PendingPermissions {
    pending: Mutex<HashMap<Uuid, oneshot::Sender<PermissionResponse>>>,
    always_allowed: Mutex<HashMap<Uuid, HashSet<String>>>,
}

impl PendingPermissions {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub async fn is_allowed(&self, session_id: Uuid, tool: &str) -> bool {
        self.always_allowed.lock().await
            .get(&session_id)
            .is_some_and(|tools| tools.contains(tool))
    }

    pub async fn request(
        &self,
        session_id: Uuid,
        tool: &str,
        description: &str,
    ) -> (Permission, oneshot::Receiver<PermissionResponse>) {
        let id = Uuid::new_v4();
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        let perm = Permission {
            id,
            session_id,
            tool: tool.to_string(),
            title: format!("Allow {tool}?"),
            description: description.to_string(),
        };

        (perm, rx)
    }

    pub async fn clear_session(&self, session_id: Uuid) {
        self.always_allowed.lock().await.remove(&session_id);
    }

    pub async fn reply(
        &self,
        id: Uuid,
        session_id: Uuid,
        tool: &str,
        response: PermissionResponse,
    ) {
        if response == PermissionResponse::Always {
            self.always_allowed.lock().await
                .entry(session_id)
                .or_default()
                .insert(tool.to_string());
        }
        if let Some(tx) = self.pending.lock().await.remove(&id) {
            let _ = tx.send(response);
        }
    }
}
