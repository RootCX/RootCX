use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ClientError, RuntimeClient, check_response};

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionApprovals {
    pub revision: Option<String>,
    pub backend_digest: Option<String>,
    pub installation_id: String,
    pub actions: Vec<ActionApproval>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionApproval {
    pub id: String,
    // Preserve the entire authority declaration so new access cannot be hidden during review.
    pub authority: Value,
    pub approval_id: Option<String>,
    pub status: ActionApprovalStatus,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ActionApprovalStatus {
    Pending,
    Approved,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionApprovalRequest {
    pub revision: String,
    pub backend_digest: String,
    pub installation_id: String,
}

impl RuntimeClient {
    pub async fn list_action_approvals(
        &self,
        app_id: &str,
    ) -> Result<ActionApprovals, ClientError> {
        let url = self.action_approvals_url(app_id, None)?;
        let resp = self.authed(self.client.get(url)).send().await?;
        check_response(resp).await?.json().await.map_err(Into::into)
    }

    /// Submit the exact reviewed snapshot; never refresh or retry a stale approval.
    pub async fn approve_action(
        &self,
        app_id: &str,
        action_id: &str,
        reviewed: &ActionApprovalRequest,
    ) -> Result<(), ClientError> {
        let url = self.action_approvals_url(app_id, Some(action_id))?;
        let resp = self
            .authed(self.client.post(url))
            .json(reviewed)
            .send()
            .await?;
        check_response(resp).await?;
        Ok(())
    }

    pub async fn revoke_action_approval(
        &self,
        app_id: &str,
        action_id: &str,
    ) -> Result<(), ClientError> {
        let url = self.action_approvals_url(app_id, Some(action_id))?;
        let resp = self.authed(self.client.delete(url)).send().await?;
        check_response(resp).await?;
        Ok(())
    }

    fn action_approvals_url(
        &self,
        app_id: &str,
        action_id: Option<&str>,
    ) -> Result<reqwest::Url, ClientError> {
        let invalid = |message: &str| ClientError::Api {
            status: 400,
            message: message.into(),
        };
        let mut url =
            reqwest::Url::parse(&self.api("/apps")).map_err(|_| invalid("invalid Core URL"))?;
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| invalid("invalid Core URL"))?;
        for segment in std::iter::once(app_id).chain(action_id) {
            if segment.is_empty() || matches!(segment, "." | "..") {
                return Err(invalid("app and action IDs must be nonempty path segments"));
            }
        }
        segments.push(app_id).push("action-approvals");
        if let Some(action_id) = action_id {
            segments.push(action_id);
        }
        drop(segments);
        Ok(url)
    }
}
