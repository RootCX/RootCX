//! Operator-owned OAuth applications; never writable by tenant administrators.
use serde_json::{json, Value};
use crate::api_error::ApiError;

pub fn enabled(id: &str) -> bool {
    id == "google_drive" && std::env::var("ROOTCX_MANAGED_GOOGLE_OAUTH").as_deref() == Ok("true")
}

pub fn config(id: &str) -> Result<Option<Value>, ApiError> {
    if !enabled(id) { return Ok(None); }
    let read = |key| std::env::var(key).ok().filter(|s| !s.trim().is_empty())
        .ok_or_else(|| ApiError::BadRequest("managed OAuth is not configured by the operator".into()));
    Ok(Some(json!({
        "clientId": read("ROOTCX_GOOGLE_CLIENT_ID")?,
        "clientSecret": read("ROOTCX_GOOGLE_CLIENT_SECRET")?,
        "tokenUrl": "https://oauth2.googleapis.com/token",
        "tokenAuthMethod": "body", "tokenEncoding": "form"
    })))
}

pub fn reject_override(id: &str) -> Result<(), ApiError> {
    if enabled(id) { return Err(ApiError::Forbidden("OAuth is managed by the operator".into())); }
    Ok(())
}

pub fn worker_config(id: &str, config: Value) -> Value {
    if enabled(id) { json!({}) } else { config }
}

pub fn start(config: &Value, callback: &str, state: &str, challenge: &str) -> Value {
    let mut url = reqwest::Url::parse("https://accounts.google.com/o/oauth2/v2/auth").unwrap();
    url.query_pairs_mut().extend_pairs([
        ("client_id", config["clientId"].as_str().unwrap_or("")),
        ("redirect_uri", callback), ("state", state), ("response_type", "code"),
        ("code_challenge", challenge), ("code_challenge_method", "S256"),
        ("scope", "openid email https://www.googleapis.com/auth/drive"),
        ("access_type", "offline"), ("prompt", "select_account consent"),
    ]);
    json!({ "type": "redirect", "url": url.as_str() })
}

async fn response_json(response: reqwest::Response) -> Result<Value, ApiError> {
    if !response.status().is_success() { return Err(ApiError::BadRequest("Google authorization or permissions rejected".into())); }
    let mut response = response;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| ApiError::BadRequest("Google response interrupted".into()))? {
        if bytes.len() + chunk.len() > 1_048_576 { return Err(ApiError::BadRequest("Google response too large".into())); }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| ApiError::BadRequest("Invalid Google response".into()))
}

pub async fn callback(config: &Value, query: &std::collections::HashMap<String, String>, callback: &str, verifier: &str) -> Result<Value, ApiError> {
    if query.contains_key("error") { return Err(ApiError::BadRequest("Google authorization declined".into())); }
    let code = query.get("code").ok_or_else(|| ApiError::BadRequest("Missing authorization code".into()))?;
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::none()).build().map_err(|_| ApiError::Internal("OAuth client unavailable".into()))?;
    let network_error = |_| ApiError::BadRequest("Google is temporarily unavailable".into());
    let token = response_json(client.post("https://oauth2.googleapis.com/token").form(&[
        ("client_id", config["clientId"].as_str().unwrap_or("")),
        ("client_secret", config["clientSecret"].as_str().unwrap_or("")),
        ("code", code), ("code_verifier", verifier), ("redirect_uri", callback),
        ("grant_type", "authorization_code"),
    ]).send().await.map_err(network_error)?).await?;
    let (access, refresh) = validate_token(&token)?;
    let (id, email) = verify_access(access).await?;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    Ok(json!({ "credentials": { "accessToken": access, "refreshToken": refresh,
        "_expiresAt": now + token["expires_in"].as_u64().unwrap_or(300).min(86400) },
        "label": format!("{email} [{id}]") }))
}

pub async fn verify_access(access: &str) -> Result<(String, String), ApiError> {
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::none()).build().map_err(|_| ApiError::Internal("OAuth client unavailable".into()))?;
    let network_error = |_| ApiError::BadRequest("Google is temporarily unavailable".into());
    response_json(client.get("https://www.googleapis.com/drive/v3/about")
        .query(&[("fields", "user(permissionId)")]).bearer_auth(access)
        .send().await.map_err(network_error)?).await?;
    let user = response_json(client.get("https://openidconnect.googleapis.com/v1/userinfo")
        .bearer_auth(access).send().await.map_err(network_error)?).await?;
    let id = user["sub"].as_str().filter(|v| !v.is_empty()).ok_or_else(|| ApiError::BadRequest("Missing Google identity".into()))?;
    let email = user["email"].as_str().filter(|v| !v.is_empty()).ok_or_else(|| ApiError::BadRequest("Missing Google email".into()))?;
    Ok((id.to_string(), email.to_string()))
}

fn validate_token(token: &Value) -> Result<(&str, &str), ApiError> {
    let field = |key| token[key].as_str().filter(|v| !v.is_empty())
        .ok_or_else(|| ApiError::BadRequest("Google did not grant durable access; reconnect and allow Drive".into()));
    if !field("scope")?.split_whitespace().any(|s| s == "https://www.googleapis.com/auth/drive") {
        return Err(ApiError::BadRequest("Google Drive permission was not granted".into()));
    }
    Ok((field("access_token")?, field("refresh_token")?))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn partial_consent_and_nonrenewable_tokens_are_rejected() {
        let valid = json!({"access_token":"access", "refresh_token":"refresh", "scope":"openid email https://www.googleapis.com/auth/drive"});
        assert!(validate_token(&valid).is_ok());
        for field in ["access_token", "refresh_token", "scope"] {
            for value in [Value::Null, json!(""), json!(42)] {
                let mut token = valid.clone(); token[field] = value;
                assert!(validate_token(&token).is_err(), "{field}: {token}");
            }
        }
        let mut partial = valid;
        partial["scope"] = json!("openid email https://www.googleapis.com/auth/drive.readonly");
        assert!(validate_token(&partial).is_err());
    }
}
