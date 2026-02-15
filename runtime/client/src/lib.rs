use rootcx_shared_types::{AppManifest, InstalledApp, OsStatus};
use serde_json::Value as JsonValue;

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("API error ({status}): {message}")]
    Api { status: u16, message: String },
}

/// HTTP client for the RootCX Runtime daemon.
#[derive(Clone)]
pub struct RuntimeClient {
    base_url: String,
    client: reqwest::Client,
}

impl RuntimeClient {
    pub fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client: reqwest::Client::new(),
        }
    }

    /// Check if the daemon is reachable.
    pub async fn is_available(&self) -> bool {
        self.client
            .get(format!("{}/health", self.base_url))
            .send()
            .await
            .is_ok()
    }

    /// Get the runtime status.
    pub async fn status(&self) -> Result<OsStatus, ClientError> {
        let resp = self
            .client
            .get(format!("{}/api/v1/status", self.base_url))
            .send()
            .await?;
        check_response(resp).await?.json().await.map_err(Into::into)
    }

    /// Install an app from a manifest.
    pub async fn install_app(&self, manifest: &AppManifest) -> Result<String, ClientError> {
        let resp = self
            .client
            .post(format!("{}/api/v1/apps", self.base_url))
            .json(manifest)
            .send()
            .await?;
        let body: JsonValue = check_response(resp).await?.json().await?;
        Ok(body["message"].as_str().unwrap_or("ok").to_string())
    }

    /// List all installed apps.
    pub async fn list_apps(&self) -> Result<Vec<InstalledApp>, ClientError> {
        let resp = self
            .client
            .get(format!("{}/api/v1/apps", self.base_url))
            .send()
            .await?;
        check_response(resp).await?.json().await.map_err(Into::into)
    }

    /// Uninstall an app by ID.
    pub async fn uninstall_app(&self, app_id: &str) -> Result<(), ClientError> {
        let resp = self
            .client
            .delete(format!("{}/api/v1/apps/{}", self.base_url, app_id))
            .send()
            .await?;
        check_response(resp).await?;
        Ok(())
    }

    /// List records in a collection.
    pub async fn list_records(
        &self,
        app_id: &str,
        entity: &str,
    ) -> Result<Vec<JsonValue>, ClientError> {
        let resp = self
            .client
            .get(format!(
                "{}/api/v1/apps/{}/collections/{}",
                self.base_url, app_id, entity
            ))
            .send()
            .await?;
        check_response(resp).await?.json().await.map_err(Into::into)
    }

    /// Create a record in a collection.
    pub async fn create_record(
        &self,
        app_id: &str,
        entity: &str,
        data: &JsonValue,
    ) -> Result<JsonValue, ClientError> {
        let resp = self
            .client
            .post(format!(
                "{}/api/v1/apps/{}/collections/{}",
                self.base_url, app_id, entity
            ))
            .json(data)
            .send()
            .await?;
        check_response(resp).await?.json().await.map_err(Into::into)
    }

    /// Get a record by ID.
    pub async fn get_record(
        &self,
        app_id: &str,
        entity: &str,
        id: &str,
    ) -> Result<JsonValue, ClientError> {
        let resp = self
            .client
            .get(format!(
                "{}/api/v1/apps/{}/collections/{}/{}",
                self.base_url, app_id, entity, id
            ))
            .send()
            .await?;
        check_response(resp).await?.json().await.map_err(Into::into)
    }

    /// Update a record by ID (partial update).
    pub async fn update_record(
        &self,
        app_id: &str,
        entity: &str,
        id: &str,
        data: &JsonValue,
    ) -> Result<JsonValue, ClientError> {
        let resp = self
            .client
            .patch(format!(
                "{}/api/v1/apps/{}/collections/{}/{}",
                self.base_url, app_id, entity, id
            ))
            .json(data)
            .send()
            .await?;
        check_response(resp).await?.json().await.map_err(Into::into)
    }

    /// Delete a record by ID.
    pub async fn delete_record(
        &self,
        app_id: &str,
        entity: &str,
        id: &str,
    ) -> Result<(), ClientError> {
        let resp = self
            .client
            .delete(format!(
                "{}/api/v1/apps/{}/collections/{}/{}",
                self.base_url, app_id, entity, id
            ))
            .send()
            .await?;
        check_response(resp).await?;
        Ok(())
    }
}

async fn check_response(resp: reqwest::Response) -> Result<reqwest::Response, ClientError> {
    if resp.status().is_success() {
        Ok(resp)
    } else {
        let status = resp.status().as_u16();
        let body: JsonValue = resp.json().await.unwrap_or_default();
        let message = body["error"]
            .as_str()
            .unwrap_or("unknown error")
            .to_string();
        Err(ClientError::Api { status, message })
    }
}
