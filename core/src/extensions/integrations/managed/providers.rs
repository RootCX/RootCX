//! OAuth clients configured by the operator. Secrets never enter plugin RPCs.
use std::collections::HashMap;
use serde_json::{json, Value};
use crate::api_error::ApiError;

pub(super) struct Provider {
    pub id: &'static str,
    namespace: &'static str,
    authorize: &'static str,
    manifest: &'static str,
    pkce: bool,
}

pub(super) fn provider(id: &str) -> Option<Provider> {
    let (namespace, authorize, manifest, pkce) = match id {
        "notion" => ("NOTION", "https://api.notion.com/v1/oauth/authorize", include_str!("../../../../resources/integrations/notion/manifest.json"), false),
        "hubspot" => ("HUBSPOT", "https://app.hubspot.com/oauth/authorize", include_str!("../../../../resources/integrations/hubspot/manifest.json"), false),
        "salesforce" => ("SALESFORCE", "https://login.salesforce.com/services/oauth2/authorize", include_str!("../../../../resources/integrations/salesforce/manifest.json"), true),
        "airtable" => ("AIRTABLE", "https://airtable.com/oauth2/v1/authorize", include_str!("../../../../resources/integrations/airtable/manifest.json"), true),
        "slack" => ("SLACK", "https://slack.com/oauth/v2/authorize", include_str!("../../../../resources/integrations/slack/manifest.json"), false),
        "gmail" => ("GMAIL", "https://accounts.google.com/o/oauth2/v2/auth", include_str!("../../../../resources/integrations/gmail/manifest.json"), true),
        "google_calendar" => ("GOOGLE_CALENDAR", "https://accounts.google.com/o/oauth2/v2/auth", include_str!("../../../../resources/integrations/google_calendar/manifest.json"), true),
        "google_sheets" => ("GOOGLE_SHEETS", "https://accounts.google.com/o/oauth2/v2/auth", include_str!("../../../../resources/integrations/google_sheets/manifest.json"), true),
        _ => return None,
    };
    let id = match namespace { "NOTION" => "notion", "HUBSPOT" => "hubspot", "SALESFORCE" => "salesforce", "AIRTABLE" => "airtable", "SLACK" => "slack", "GMAIL" => "gmail", "GOOGLE_CALENDAR" => "google_calendar", _ => "google_sheets" };
    Some(Provider { id, namespace, authorize, manifest, pkce })
}

fn error(message: &str) -> ApiError { ApiError::BadRequest(message.into()) }
fn field<'a>(value: &'a Value, key: &str) -> Result<&'a str, ApiError> {
    value[key].as_str().filter(|s| !s.trim().is_empty() && !s.contains(['\r', '\n']))
        .ok_or_else(|| error("Provider returned incomplete authorization data"))
}

impl Provider {
    fn google(&self) -> bool { matches!(self.id, "gmail" | "google_calendar" | "google_sheets") }
    fn oauth(&self) -> Value {
        // Compiled assets, never a tenant-installed manifest or provider-supplied URL.
        serde_json::from_str::<Value>(self.manifest).expect("bundled manifest")["userAuth"]["oauth"].clone()
    }
    pub(super) fn enabled(&self) -> bool {
        std::env::var(format!("ROOTCX_MANAGED_{}_OAUTH", self.namespace)).as_deref() == Ok("true")
    }
    pub(super) fn config(&self) -> Result<Value, ApiError> {
        self.config_with(|key| std::env::var(key).ok())
    }
    fn config_with(&self, read: impl Fn(&str) -> Option<String>) -> Result<Value, ApiError> {
        let namespace = if self.google() { "GOOGLE" } else { self.namespace };
        let secret = |suffix| read(&format!("ROOTCX_{namespace}_{suffix}"))
            .filter(|s| !s.trim().is_empty()).ok_or_else(|| error("Managed OAuth is not configured by the operator"));
        let mut config = self.oauth();
        config["clientId"] = json!(secret("CLIENT_ID")?);
        config["clientSecret"] = json!(secret("CLIENT_SECRET")?);
        Ok(config)
    }
    pub(super) fn start(&self, config: &Value, callback: &str, state: &str, challenge: &str) -> Value {
        let oauth = self.oauth();
        let scopes: Vec<_> = oauth["scopes"].as_array().expect("bundled scopes").iter().filter_map(Value::as_str).collect();
        let mut url = reqwest::Url::parse(self.authorize).expect("provider URL");
        let mut query = url.query_pairs_mut();
        query.extend_pairs([("client_id", config["clientId"].as_str().unwrap_or("")), ("redirect_uri", callback), ("state", state), ("response_type", "code")]);
        if !scopes.is_empty() { query.append_pair("scope", &scopes.join(if self.id == "slack" { "," } else { " " })); }
        if self.pkce { query.extend_pairs([("code_challenge", challenge), ("code_challenge_method", "S256")]); }
        if self.google() { query.extend_pairs([("access_type", "offline"), ("prompt", "select_account consent")]); }
        if self.id == "notion" { query.append_pair("owner", "user"); }
        drop(query);
        json!({"type": "redirect", "url": url.as_str()})
    }
    pub(super) async fn callback(&self, config: &Value, query: &HashMap<String, String>, callback: &str, verifier: &str) -> Result<Value, ApiError> {
        if query.contains_key("error") { return Err(error("Provider authorization declined")); }
        let code = query.get("code").filter(|s| !s.is_empty()).ok_or_else(|| error("Missing authorization code"))?;
        let token = response(self.exchange_request(config, code, callback, verifier)?.send().await).await?;
        let credentials = self.credentials(&token)?;
        let (id, label) = self.verify(&credentials).await?;
        Ok(json!({"credentials": credentials, "label": format!("{label} [{id}]")}))
    }
    fn exchange_request(&self, config: &Value, code: &str, callback: &str, verifier: &str) -> Result<reqwest::RequestBuilder, ApiError> {
        let oauth = self.oauth();
        let mut body = json!({"grant_type": "authorization_code", "code": code, "redirect_uri": callback});
        if self.pkce {
            if verifier.is_empty() { return Err(error("PKCE verifier missing")); }
            body["code_verifier"] = json!(verifier);
        }
        let mut request = client()?.post(field(&oauth, "tokenUrl")?);
        if oauth["tokenAuthMethod"] == "basic" {
            request = request.basic_auth(field(config, "clientId")?, Some(field(config, "clientSecret")?));
        } else {
            body["client_id"] = json!(field(config, "clientId")?);
            body["client_secret"] = json!(field(config, "clientSecret")?);
        }
        Ok(if oauth["tokenEncoding"] == "json" { request.json(&body) } else { request.form(&body) })
    }
    fn credentials(&self, token: &Value) -> Result<Value, ApiError> {
        if token.get("error").is_some() || token["ok"] == false { return Err(error("Provider authorization rejected")); }
        let mut credentials = json!({"accessToken": field(token, "access_token")?});
        if self.id != "slack" || token.get("refresh_token").is_some() {
            credentials["refreshToken"] = json!(field(token, "refresh_token")?);
        }
        if let Some(expiry) = token.get("expires_in") {
            let lifetime = expiry.as_u64().filter(|n| *n > 0).ok_or_else(|| error("Provider returned invalid token lifetime"))?;
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
            credentials["_expiresAt"] = json!(now + lifetime.min(31_536_000));
        }
        if self.google() {
            let oauth = self.oauth();
            let granted: Vec<_> = field(token, "scope")?.split_whitespace().collect();
            if oauth["scopes"].as_array().unwrap().iter().filter_map(Value::as_str)
                .filter(|scope| scope.starts_with("https://www.googleapis.com/auth/"))
                .any(|scope| !granted.contains(&scope)) { return Err(error("Required Google permissions were not granted")); }
        }
        if self.id == "salesforce" {
            let instance = field(token, "instance_url")?;
            salesforce_url(instance)?;
            credentials["instanceUrl"] = json!(instance);
        }
        Ok(credentials)
    }
    pub(super) async fn verify(&self, credentials: &Value) -> Result<(String, String), ApiError> {
        let access = field(credentials, "accessToken")?;
        let url = match self.id {
            "notion" => "https://api.notion.com/v1/users/me".to_string(),
            "hubspot" => "https://api.hubapi.com/account-info/v3/details".to_string(),
            "airtable" => "https://api.airtable.com/v0/meta/whoami".to_string(),
            "slack" => "https://slack.com/api/auth.test".to_string(),
            "salesforce" => salesforce_url(field(credentials, "instanceUrl")?)?.join("services/oauth2/userinfo").unwrap().to_string(),
            _ => "https://openidconnect.googleapis.com/v1/userinfo".to_string(),
        };
        let url = reqwest::Url::parse(&url).map_err(|_| error("Invalid provider URL"))?;
        let addrs = crate::tools::http_request::resolve_allowed_url(&url).await.map_err(|_| error("Provider address unavailable"))?;
        let client = crate::tools::http_request::build_client(20_000, &url, &addrs).map_err(|_| error("Provider unavailable"))?;
        let mut request = client.get(url).bearer_auth(access);
        if self.id == "notion" { request = request.header("Notion-Version", "2025-09-03"); }
        let account = response(request.send().await).await?;
        let identity = match self.id {
            "notion" => (field(&account, "id")?.to_string(), account["bot"]["workspace_name"].as_str().unwrap_or("Notion").to_string()),
            "hubspot" => {
                let id = account["portalId"].as_u64().filter(|n| *n > 0).ok_or_else(|| error("Missing HubSpot account"))?.to_string();
                (id.clone(), format!("HubSpot {id}"))
            },
            "airtable" => (field(&account, "id")?.to_string(), account["email"].as_str().unwrap_or(field(&account, "id")?).to_string()),
            "slack" => {
                if account["ok"] != true { return Err(error("Slack authorization rejected")); }
                (format!("{}:{}", field(&account, "team_id")?, field(&account, "user_id")?), format!("{} / {}", field(&account, "team")?, field(&account, "user")?))
            },
            "salesforce" => (format!("{}:{}", field(&account, "organization_id")?, field(&account, "user_id")?), account["preferred_username"].as_str().unwrap_or(field(&account, "user_id")?).to_string()),
            _ => (field(&account, "sub")?.to_string(), field(&account, "email")?.to_string()),
        };
        Ok(identity)
    }
}
fn client() -> Result<reqwest::Client, ApiError> {
    reqwest::Client::builder().timeout(std::time::Duration::from_secs(20)).redirect(reqwest::redirect::Policy::none())
        .build().map_err(|_| error("OAuth client unavailable"))
}
async fn response(result: Result<reqwest::Response, reqwest::Error>) -> Result<Value, ApiError> {
    let mut response = result.map_err(|_| error("Provider temporarily unavailable"))?;
    if !response.status().is_success() { return Err(error("Provider authorization or permissions rejected")); }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| error("Provider response interrupted"))? {
        if bytes.len() + chunk.len() > 1_048_576 { return Err(error("Provider response too large")); }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| error("Invalid provider response"))
}
fn salesforce_url(raw: &str) -> Result<reqwest::Url, ApiError> {
    let url = reqwest::Url::parse(raw).map_err(|_| error("Invalid Salesforce instance"))?;
    if url.scheme() != "https" || url.port().is_some() || !url.username().is_empty() || url.password().is_some()
        || url.path() != "/" || url.query().is_some() || url.fragment().is_some()
        || !url.host_str().is_some_and(|host| host.ends_with(".salesforce.com")) { return Err(error("Invalid Salesforce instance")); }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    const IDS: [&str; 8] = ["notion", "hubspot", "salesforce", "airtable", "slack", "gmail", "google_calendar", "google_sheets"];

    #[test]
    fn provider_secrets_use_operator_namespaces_and_fail_closed() {
        for id in IDS {
            let p = provider(id).unwrap();
            let namespace = if p.google() { "GOOGLE" } else { p.namespace };
            let cfg = p.config_with(|key| match key.strip_prefix(&format!("ROOTCX_{namespace}_")) {
                Some("CLIENT_ID") => Some("client".into()), Some("CLIENT_SECRET") => Some("secret".into()), _ => None,
            }).unwrap();
            assert_eq!(cfg["clientId"], "client", "{id}");
            assert_eq!(cfg["clientSecret"], "secret", "{id}");
            assert!(p.config_with(|_| None).is_err(), "{id}");
            assert!(p.config_with(|_| Some(" ".into())).is_err(), "{id}");
        }
        for unknown in ["google_drive", "stripe", "tenant_custom", ""] { assert!(provider(unknown).is_none()); }
    }

    #[test]
    fn native_authorization_keeps_provider_protocol_and_secrets_server_side() {
        for id in IDS {
            let p = provider(id).unwrap();
            let cfg = json!({"clientId":"fixture-client", "clientSecret":"fixture-secret", "tokenUrl":"https://attacker.example"});
            let start = p.start(&cfg, "https://app.example/cb", "state", "challenge");
            assert!(!start.to_string().contains("fixture-secret"), "{id}");
            let url = reqwest::Url::parse(start["url"].as_str().unwrap()).unwrap();
            let query: HashMap<_, _> = url.query_pairs().into_owned().collect();
            assert_eq!(query["state"], "state", "{id}");
            assert_eq!(query.get("code_challenge_method").map(String::as_str), if p.pkce { Some("S256") } else { None }, "{id}");
            if id == "notion" { assert_eq!(query["owner"], "user"); }
            if p.google() { assert_eq!(query["access_type"], "offline"); }
            let request = p.exchange_request(&cfg, "code", "https://app.example/cb", "verifier").unwrap().build().unwrap();
            assert_ne!(request.url().host_str(), Some("attacker.example"), "{id}");
            let raw = std::str::from_utf8(request.body().unwrap().as_bytes().unwrap()).unwrap();
            let body: Value = if id == "notion" { serde_json::from_str(raw).unwrap() }
                else { json!(url::form_urlencoded::parse(raw.as_bytes()).into_owned().collect::<HashMap<_, _>>()) };
            assert_eq!(body["code"], "code", "{id}");
            assert_eq!(body.get("code_verifier").and_then(Value::as_str), if p.pkce { Some("verifier") } else { None }, "{id}");
            if matches!(id, "notion" | "airtable") {
                assert!(request.headers().contains_key("authorization"), "{id}");
                assert!(body.get("client_secret").is_none(), "{id}");
            } else { assert_eq!(body["client_secret"], "fixture-secret", "{id}"); }
            if p.pkce { assert!(p.exchange_request(&cfg, "code", "cb", "").is_err(), "{id}"); }
        }
    }

    #[test]
    fn authorization_rejects_incomplete_grants_and_preserves_refresh_metadata() {
        for id in IDS {
            let p = provider(id).unwrap();
            let scopes = p.oauth()["scopes"].as_array().unwrap().iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" ");
            let token = json!({"access_token":"access", "refresh_token":"refresh", "expires_in":3600, "scope":scopes, "instance_url":"https://fixture.my.salesforce.com"});
            let credentials = p.credentials(&token).unwrap();
            assert_eq!(credentials["refreshToken"], "refresh", "{id}");
            for (key, value) in [("access_token", json!(" ")), ("refresh_token", json!("")), ("expires_in", json!(0)), ("expires_in", json!(-1)), ("ok", json!(false)), ("error", json!("rejected"))] {
                let mut bad = token.clone(); bad[key] = value;
                assert!(p.credentials(&bad).is_err(), "{id}: {key}");
            }
            let mut no_refresh = token.clone(); no_refresh.as_object_mut().unwrap().remove("refresh_token");
            assert_eq!(p.credentials(&no_refresh).is_ok(), id == "slack", "{id}");
            if p.google() {
                let mut partial = token.clone(); partial["scope"] = json!("openid email");
                assert!(p.credentials(&partial).is_err(), "{id}");
            }
            if id == "salesforce" { assert_eq!(credentials["instanceUrl"], token["instance_url"]); }
        }
    }

    #[test]
    fn salesforce_identity_destination_refuses_untrusted_origins() {
        for bad in ["http://org.salesforce.com", "https://org.salesforce.com.evil.test", "https://salesforce.com", "https://user@org.salesforce.com", "https://org.salesforce.com/path", "https://org.salesforce.com?x=1", "https://org.salesforce.com#x", "https://org.salesforce.com:444", "https://127.0.0.1"] {
            assert!(salesforce_url(bad).is_err(), "{bad}");
        }
        assert!(salesforce_url("https://org.my.salesforce.com").is_ok());
    }
}
