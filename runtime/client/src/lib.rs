use std::collections::HashMap;
use std::sync::Arc;

use rootcx_types::{AiConfig, AppManifest, InstalledApp, OsStatus, SchemaVerification};
use serde_json::Value as JsonValue;

pub mod daemon;
pub use daemon::{RuntimeStatus, ensure_runtime, prompt_runtime_install, deploy_bundled_backend};

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("API error ({status}): {message}")]
    Api { status: u16, message: String },

    #[error("failed to start runtime: {0}")]
    RuntimeStart(String),
}

#[derive(Clone)]
pub struct RuntimeClient {
    base_url: String,
    client: reqwest::Client,
    token: Arc<std::sync::RwLock<Option<String>>>,
}

impl RuntimeClient {
    pub fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client: reqwest::Client::new(),
            token: Arc::new(std::sync::RwLock::new(None)),
        }
    }

    fn api(&self, path: &str) -> String {
        format!("{}/api/v1{path}", self.base_url)
    }

    fn authed(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(ref t) = *self.token.read().unwrap() { req.bearer_auth(t) } else { req }
    }

    pub fn set_token(&self, token: Option<String>) {
        *self.token.write().unwrap() = token;
    }

    pub fn token(&self) -> Option<String> {
        self.token.read().unwrap().clone()
    }

    pub async fn authenticate(&self, username: &str, password: &str) -> Result<(), ClientError> {
        let creds = serde_json::json!({ "username": username, "password": password });
        let _ = self.client.post(self.api("/auth/register")).json(&creds).send().await;
        let resp = self.client.post(self.api("/auth/login")).json(&creds).send().await?;
        let body: JsonValue = check_response(resp).await?.json().await?;
        let token = body["accessToken"]
            .as_str()
            .ok_or_else(|| ClientError::Api { status: 0, message: "missing accessToken".into() })?;
        *self.token.write().unwrap() = Some(token.to_string());
        Ok(())
    }

    pub async fn is_available(&self) -> bool {
        self.client.get(format!("{}/health", self.base_url)).send().await.is_ok()
    }

    pub async fn status(&self) -> Result<OsStatus, ClientError> {
        let resp = self.authed(self.client.get(self.api("/status"))).send().await?;
        check_response(resp).await?.json().await.map_err(Into::into)
    }

    pub async fn install_app(&self, manifest: &AppManifest) -> Result<String, ClientError> {
        let resp = self.authed(self.client.post(self.api("/apps"))).json(manifest).send().await?;
        extract_message(resp).await
    }

    pub async fn list_apps(&self) -> Result<Vec<InstalledApp>, ClientError> {
        let resp = self.authed(self.client.get(self.api("/apps"))).send().await?;
        check_response(resp).await?.json().await.map_err(Into::into)
    }

    pub async fn uninstall_app(&self, app_id: &str) -> Result<(), ClientError> {
        let resp = self.authed(self.client.delete(self.api(&format!("/apps/{app_id}")))).send().await?;
        check_response(resp).await?;
        Ok(())
    }

    pub async fn list_records(&self, app_id: &str, entity: &str) -> Result<Vec<JsonValue>, ClientError> {
        let resp = self.authed(self.client.get(self.api(&format!("/apps/{app_id}/collections/{entity}")))).send().await?;
        check_response(resp).await?.json().await.map_err(Into::into)
    }

    pub async fn create_record(&self, app_id: &str, entity: &str, data: &JsonValue) -> Result<JsonValue, ClientError> {
        let resp = self
            .authed(self.client.post(self.api(&format!("/apps/{app_id}/collections/{entity}"))))
            .json(data)
            .send()
            .await?;
        check_response(resp).await?.json().await.map_err(Into::into)
    }

    pub async fn bulk_create_records(&self, app_id: &str, entity: &str, data: &[JsonValue]) -> Result<Vec<JsonValue>, ClientError> {
        let resp = self
            .authed(self.client.post(self.api(&format!("/apps/{app_id}/collections/{entity}/bulk"))))
            .json(&data)
            .send()
            .await?;
        check_response(resp).await?.json().await.map_err(Into::into)
    }

    pub async fn get_record(&self, app_id: &str, entity: &str, id: &str) -> Result<JsonValue, ClientError> {
        let resp = self
            .authed(self.client.get(self.api(&format!("/apps/{app_id}/collections/{entity}/{id}"))))
            .send()
            .await?;
        check_response(resp).await?.json().await.map_err(Into::into)
    }

    pub async fn update_record(
        &self,
        app_id: &str,
        entity: &str,
        id: &str,
        data: &JsonValue,
    ) -> Result<JsonValue, ClientError> {
        let resp = self
            .authed(self.client.patch(self.api(&format!("/apps/{app_id}/collections/{entity}/{id}"))))
            .json(data)
            .send()
            .await?;
        check_response(resp).await?.json().await.map_err(Into::into)
    }

    pub async fn delete_record(&self, app_id: &str, entity: &str, id: &str) -> Result<(), ClientError> {
        let resp = self
            .authed(self.client.delete(self.api(&format!("/apps/{app_id}/collections/{entity}/{id}"))))
            .send()
            .await?;
        check_response(resp).await?;
        Ok(())
    }

    pub async fn verify_schema(&self, manifest: &AppManifest) -> Result<SchemaVerification, ClientError> {
        let resp = self.authed(self.client.post(self.api("/apps/schema/verify"))).json(manifest).send().await?;
        check_response(resp).await?.json().await.map_err(Into::into)
    }

    pub async fn deploy_app(&self, app_id: &str, archive: Vec<u8>) -> Result<String, ClientError> {
        self.upload_archive(&format!("/apps/{app_id}/deploy"), archive).await
    }

    pub async fn deploy_frontend(&self, app_id: &str, archive: Vec<u8>) -> Result<String, ClientError> {
        self.upload_archive(&format!("/apps/{app_id}/frontend"), archive).await
    }

    async fn upload_archive(&self, path: &str, archive: Vec<u8>) -> Result<String, ClientError> {
        let part = reqwest::multipart::Part::bytes(archive)
            .mime_str("application/gzip")
            .map_err(ClientError::Http)?;
        let form = reqwest::multipart::Form::new().part("archive", part);
        let resp = self.authed(self.client.post(self.api(path))).multipart(form).send().await?;
        extract_message(resp).await
    }

    pub async fn start_worker(&self, app_id: &str) -> Result<String, ClientError> {
        self.worker_action(app_id, "start").await
    }

    pub async fn stop_worker(&self, app_id: &str) -> Result<String, ClientError> {
        self.worker_action(app_id, "stop").await
    }

    pub async fn worker_status(&self, app_id: &str) -> Result<String, ClientError> {
        let resp = self.authed(self.client.get(self.api(&format!("/apps/{app_id}/worker/status")))).send().await?;
        let body: JsonValue = check_response(resp).await?.json().await?;
        Ok(body["status"].as_str().unwrap_or("unknown").to_string())
    }

    pub async fn get_ai_config(&self) -> Result<Option<AiConfig>, ClientError> {
        let resp = self.authed(self.client.get(self.api("/config/ai"))).send().await?;
        if resp.status().as_u16() == 404 {
            return Ok(None);
        }
        Ok(Some(check_response(resp).await?.json().await?))
    }

    pub async fn set_ai_config(&self, config: &AiConfig) -> Result<(), ClientError> {
        let resp = self.authed(self.client.put(self.api("/config/ai"))).json(config).send().await?;
        check_response(resp).await?;
        Ok(())
    }

    pub async fn get_forge_config(&self) -> Result<JsonValue, ClientError> {
        let resp = self.authed(self.client.get(self.api("/config/ai/forge"))).send().await?;
        check_response(resp).await?.json().await.map_err(Into::into)
    }

    pub async fn get_platform_env(&self) -> Result<HashMap<String, String>, ClientError> {
        let resp = self.authed(self.client.get(self.api("/platform/secrets/env"))).send().await?;
        let body: HashMap<String, String> = check_response(resp).await?.json().await?;
        Ok(body)
    }

    pub async fn list_platform_secrets(&self) -> Result<Vec<String>, ClientError> {
        let resp = self.authed(self.client.get(self.api("/platform/secrets"))).send().await?;
        check_response(resp).await?.json().await.map_err(Into::into)
    }

    pub async fn set_platform_secret(&self, key: &str, value: &str) -> Result<(), ClientError> {
        let body = serde_json::json!({ "key": key, "value": value });
        let resp = self.authed(self.client.post(self.api("/platform/secrets"))).json(&body).send().await?;
        check_response(resp).await?;
        Ok(())
    }

    pub async fn delete_platform_secret(&self, key: &str) -> Result<(), ClientError> {
        let resp = self.authed(self.client.delete(self.api(&format!("/platform/secrets/{key}")))).send().await?;
        check_response(resp).await?;
        Ok(())
    }

    async fn worker_action(&self, app_id: &str, action: &str) -> Result<String, ClientError> {
        let resp = self
            .authed(self.client.post(self.api(&format!("/apps/{app_id}/worker/{action}"))))
            .send()
            .await?;
        extract_message(resp).await
    }
}

async fn extract_message(resp: reqwest::Response) -> Result<String, ClientError> {
    let body: JsonValue = check_response(resp).await?.json().await?;
    Ok(body["message"].as_str().unwrap_or("ok").to_string())
}

async fn check_response(resp: reqwest::Response) -> Result<reqwest::Response, ClientError> {
    if resp.status().is_success() {
        return Ok(resp);
    }
    let status = resp.status().as_u16();
    let body: JsonValue = resp.json().await.unwrap_or_default();
    let message = body["error"].as_str().unwrap_or("unknown error").to_string();
    Err(ClientError::Api { status, message })
}
