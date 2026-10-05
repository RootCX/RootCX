use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use tokio::sync::{Mutex, oneshot};
use tokio::time::Instant;

use crate::governance::execution::ApprovalOwner;

pub const APPROVAL_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRequest {
    #[serde(skip)]
    pub owner: ApprovalOwner,
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
    expires_at: Instant,
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
        let expires_at = Instant::now() + APPROVAL_TIMEOUT;
        self.pending.lock().await.insert(id, PendingEntry { request, tx, expires_at });
        rx
    }

    pub async fn reply(&self, approval_id: &str, owner: &ApprovalOwner, response: ApprovalResponse) -> bool {
        self.consume(approval_id, None, owner, response).await
    }

    pub async fn reply_for_app(
        &self, approval_id: &str, app_id: &str, owner: &ApprovalOwner, response: ApprovalResponse,
    ) -> bool {
        self.consume(approval_id, Some(app_id), owner, response).await
    }

    async fn consume(
        &self, approval_id: &str, app_id: Option<&str>, owner: &ApprovalOwner, response: ApprovalResponse,
    ) -> bool {
        let mut pending = self.pending.lock().await;
        let Some(entry) = pending.get(approval_id) else { return false; };
        // Expiry is enforced here even when the waiting worker's timer is delayed.
        if entry.expires_at <= Instant::now() {
            let entry = pending.remove(approval_id).unwrap();
            let _ = entry.tx.send(ApprovalResponse::Rejected { reason: "expired".into() });
            return false;
        }
        if entry.request.owner != *owner || app_id.is_some_and(|app| entry.request.app_id != app) { return false; }
        pending.remove(approval_id).unwrap().tx.send(response).is_ok()
    }

    pub async fn cancel(&self, approval_id: &str, reason: &str) {
        if let Some(entry) = self.pending.lock().await.remove(approval_id) {
            let _ = entry.tx.send(ApprovalResponse::Rejected { reason: reason.into() });
        }
    }

    pub async fn list(&self, app_id: &str, owner: &ApprovalOwner) -> Vec<ApprovalRequest> {
        let pending = self.pending.lock().await;
        let now = Instant::now();
        pending.values()
            .filter(|e| e.request.app_id == app_id && e.request.owner == *owner && e.expires_at > now && !e.tx.is_closed())
            .map(|e| e.request.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn request(owner: ApprovalOwner) -> ApprovalRequest {
        ApprovalRequest {
            owner, approval_id: "approval".into(), app_id: "assistant".into(),
            session_id: "session".into(), invoke_id: "invoke".into(), call_id: "call".into(),
            tool_name: "mutate_data".into(), args: serde_json::json!({"action": "create"}), reason: "confirm".into(), created_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    #[tokio::test]
    async fn only_the_bound_owner_and_app_can_see_and_consume_a_confirmation() {
        for origin in [None, Some("channel:a:chat:a".into())] {
            let approvals = PendingApprovals::new();
            let owner = ApprovalOwner { user_id: Uuid::new_v4(), origin };
            let rx = approvals.request(request(owner.clone())).await;
            for (case, app, other) in [
                ("user", "assistant", ApprovalOwner { user_id: Uuid::new_v4(), ..owner.clone() }),
                ("origin", "assistant", ApprovalOwner { origin: Some("channel:b:chat:a".into()), ..owner.clone() }),
                ("web/channel", "assistant", ApprovalOwner { origin: if owner.origin.is_some() { None } else { Some("channel:a:chat:a".into()) }, ..owner.clone() }),
                ("app", "other", owner.clone()),
            ] {
                assert!(approvals.list(app, &other).await.is_empty(), "{case}, {owner:?}");
                assert!(!approvals.reply_for_app("approval", app, &other, ApprovalResponse::Approved).await, "{case}, {owner:?}");
                if app == "assistant" {
                    assert!(!approvals.reply("approval", &other, ApprovalResponse::Approved).await, "{case}, {owner:?}");
                }
            }
            assert_eq!(approvals.list("assistant", &owner).await.len(), 1, "{owner:?}");
            assert!(approvals.reply_for_app("approval", "assistant", &owner, ApprovalResponse::Approved).await, "{owner:?}");
            assert!(matches!(rx.await, Ok(ApprovalResponse::Approved)), "{owner:?}");
            assert!(!approvals.reply("approval", &owner, ApprovalResponse::Approved).await, "replay, {owner:?}");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn confirmation_cannot_be_consumed_at_or_after_expiry() {
        for elapsed in [APPROVAL_TIMEOUT - Duration::from_nanos(1), APPROVAL_TIMEOUT, APPROVAL_TIMEOUT + Duration::from_secs(1)] {
            let approvals = PendingApprovals::new();
            let owner = ApprovalOwner { user_id: Uuid::new_v4(), origin: None };
            let rx = approvals.request(request(owner.clone())).await;
            tokio::time::advance(elapsed).await;
            let valid = elapsed < APPROVAL_TIMEOUT;
            assert_eq!(approvals.list("assistant", &owner).await.len(), usize::from(valid), "{elapsed:?}");
            assert_eq!(approvals.reply("approval", &owner, ApprovalResponse::Approved).await, valid, "{elapsed:?}");
            match rx.await {
                Ok(ApprovalResponse::Approved) => assert!(valid, "{elapsed:?}"),
                Ok(ApprovalResponse::Rejected { reason }) => {
                    assert!(!valid, "{elapsed:?}");
                    assert_eq!(reason, "expired", "{elapsed:?}");
                }
                Err(_) => panic!("expiry must reject the waiting request: {elapsed:?}"),
            }
        }
    }

    #[tokio::test]
    async fn undeliverable_confirmation_cannot_be_approved() {
        for case in ["delivery failed", "receiver dropped"] {
            let approvals = PendingApprovals::new();
            let owner = ApprovalOwner { user_id: Uuid::new_v4(), origin: Some("channel:a:chat:a".into()) };
            let rx = approvals.request(request(owner.clone())).await;
            match case {
                "delivery failed" => {
                    approvals.cancel("approval", case).await;
                    assert!(matches!(rx.await, Ok(ApprovalResponse::Rejected { reason }) if reason == case), "{case}");
                }
                _ => drop(rx),
            }
            assert!(approvals.list("assistant", &owner).await.is_empty(), "{case}");
            assert!(!approvals.reply("approval", &owner, ApprovalResponse::Approved).await, "{case}");
            assert!(!approvals.reply("approval", &owner, ApprovalResponse::Approved).await, "replay after {case}");
        }
    }
}
