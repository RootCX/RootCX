//! Stripe Apps OAuth: publisher credentials stay in Core; installers receive account-scoped tokens.
use serde_json::{json, Value};
use crate::api_error::ApiError;

pub(crate) const TOKEN_URL: &str = "https://api.stripe.com/v1/oauth/token";
// https://docs.stripe.com/stripe-apps/api-authentication/oauth#refresh-your-access-token
pub(crate) const ACCESS_LIFETIME: i64 = 3600;
fn error() -> ApiError { ApiError::BadRequest("Stripe OAuth unavailable or authorization rejected".into()) }
fn field<'a>(value: &'a Value, key: &str) -> Result<&'a str, ApiError> {
    value[key].as_str().filter(|s| !s.trim().is_empty() && !s.contains(['\r', '\n'])).ok_or_else(error)
}
fn config_with(read: impl Fn(&str) -> Option<String>) -> Result<Value, ApiError> {
    let client = read("ROOTCX_STRIPE_CLIENT_ID").filter(|s| !s.trim().is_empty() && !s.contains(['\r', '\n'])).ok_or_else(error)?;
    let key = read("ROOTCX_STRIPE_CLIENT_SECRET").ok_or_else(error)?;
    let live = match read("ROOTCX_STRIPE_OAUTH_MODE").as_deref() {
        Some("live") => true, Some("test" | "sandbox") => false, _ => return Err(error()),
    };
    if !key.starts_with(if live { "sk_live_" } else { "sk_test_" }) || key.len() <= 8 || key.contains(['\r', '\n']) { return Err(error()); }
    let external_test = read("ROOTCX_STRIPE_EXTERNAL_TEST_LINK_ID");
    if external_test.as_ref().is_some_and(|id| live || !id.strip_prefix("chnlink_")
        .is_some_and(|suffix| !suffix.is_empty() && suffix.len() <= 128 && suffix.bytes().all(|c| c.is_ascii_alphanumeric()))) {
        return Err(error());
    }
    Ok(json!({"stripeExternalTestLinkId": external_test, "clientId": client, "clientSecret": key, "tokenUrl": TOKEN_URL,
        "tokenAuthMethod": "stripe-apps", "tokenEncoding": "form", "stripeLivemode": live}))
}
pub(super) fn config() -> Result<Value, ApiError> { config_with(|key| std::env::var(key).ok()) }
pub(super) fn enabled() -> bool {
    std::env::var("ROOTCX_MANAGED_STRIPE_OAUTH").as_deref() == Ok("true") && config().is_ok()
}
pub(super) fn start(config: &Value, callback: &str, state: &str) -> Value {
    let mut url = reqwest::Url::parse("https://marketplace.stripe.com/oauth/v2/authorize").unwrap();
    if let Some(id) = config["stripeExternalTestLinkId"].as_str() {
        url.set_path(&format!("/oauth/v2/{id}/authorize"));
    }
    url.query_pairs_mut().extend_pairs([
        ("client_id", config["clientId"].as_str().unwrap_or("")), ("redirect_uri", callback), ("state", state),
    ]);
    json!({"type": "redirect", "url": url.as_str()})
}
fn client() -> Result<reqwest::Client, ApiError> {
    reqwest::Client::builder().timeout(std::time::Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::none()).build().map_err(|_| error())
}
fn token_request(config: &Value, grant: &str, key: &str, value: &str) -> Result<reqwest::RequestBuilder, ApiError> {
    Ok(client()?.post(TOKEN_URL).basic_auth(field(config, "clientSecret")?, Some(""))
        .form(&[("grant_type", grant), (key, value)]))
}
async fn response(result: Result<reqwest::Response, reqwest::Error>) -> Result<Value, ApiError> {
    let mut response = result.map_err(|_| error())?;
    if !response.status().is_success() { return Err(error()); }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| error())? {
        if body.len() + chunk.len() > 1_048_576 { return Err(error()); }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| error())
}
fn credentials(config: &Value, token: &Value) -> Result<Value, ApiError> {
    let account = field(token, "stripe_user_id")?;
    if !account.starts_with("acct_") || token["scope"] != "stripe_apps" || token["token_type"] != "bearer"
        || token["livemode"].as_bool().is_none() || token["livemode"] != config["stripeLivemode"] || token.get("error").is_some() { return Err(error()); }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|_| error())?.as_secs();
    Ok(json!({"accessToken": field(token, "access_token")?, "refreshToken": field(token, "refresh_token")?,
        "stripeAccountId": account, "stripeLivemode": token["livemode"], "_expiresAt": now + ACCESS_LIFETIME as u64}))
}
pub(super) async fn verify(credentials: &Value) -> Result<(String, String), ApiError> {
    let account = response(client()?.get("https://api.stripe.com/v1/account")
        .bearer_auth(field(credentials, "accessToken")?).send().await).await?;
    let id = field(&account, "id")?;
    if credentials.get("stripeAccountId").is_some_and(|expected| expected.as_str() != Some(id)) { return Err(error()); }
    Ok((id.to_string(), account["business_profile"]["name"].as_str().filter(|v| !v.is_empty()).unwrap_or(id).to_string()))
}
pub(super) async fn callback(config: &Value, query: &std::collections::HashMap<String, String>) -> Result<Value, ApiError> {
    if query.contains_key("error") { return Err(error()); }
    let code = query.get("code").filter(|s| !s.is_empty()).ok_or_else(error)?;
    let token = response(token_request(config, "authorization_code", "code", code)?.send().await).await?;
    let credentials = credentials(config, &token)?;
    let (id, label) = verify(&credentials).await?;
    Ok(json!({"credentials": credentials, "label": format!("{label} [{id}]")}))
}

/// Fixed Stripe Apps endpoint and publisher Basic authentication; never tenant-selected hosts.
pub(crate) async fn refresh(config: &Value, refresh: &str, account: Option<&str>) -> Result<Value, super::super::oauth::RenewError> {
    use super::super::oauth::RenewError;
    if config["tokenUrl"] != TOKEN_URL || config["tokenEncoding"] != "form" {
        return Err(RenewError::Transient("Invalid Stripe Apps token endpoint".into()));
    }
    let response = token_request(config, "refresh_token", "refresh_token", refresh)
        .map_err(|_| RenewError::Transient("Stripe publisher unavailable".into()))?
        .send().await.map_err(|_| RenewError::Transient("Stripe token endpoint unavailable".into()))?;
    let status = response.status();
    let mut response = response;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| RenewError::Transient("Stripe response interrupted".into()))? {
        if bytes.len() + chunk.len() > 1_048_576 { return Err(RenewError::Transient("Stripe response too large".into())); }
        bytes.extend_from_slice(&chunk);
    }
    let token: Value = serde_json::from_slice(&bytes).map_err(|_| RenewError::Transient("Invalid Stripe response".into()))?;
    interpret_refresh(status.as_u16(), config, &token, account)
}
fn interpret_refresh(status: u16, config: &Value, token: &Value, account: Option<&str>) -> Result<Value, super::super::oauth::RenewError> {
    use super::super::oauth::RenewError;
    if !(200..300).contains(&status) || token.get("error").is_some() {
        return Err(if matches!(status, 400 | 401) && token["error"] == "invalid_grant" {
            RenewError::Rejected("Stripe refresh grant rejected".into())
        } else { RenewError::Transient("Stripe token exchange rejected".into()) });
    }
    refreshed_credentials(config, token, account)
}
fn refreshed_credentials(config: &Value, token: &Value, account: Option<&str>) -> Result<Value, super::super::oauth::RenewError> {
    use super::super::oauth::RenewError;
    if account.is_none() || token["stripe_user_id"].as_str() != account {
        return Err(RenewError::Transient("Stripe refreshed account mismatch".into()));
    }
    credentials(config, token).map_err(|_| RenewError::Transient("Invalid Stripe renewed grant".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn publisher_mode_and_key_must_agree() {
        for (mode, key, valid) in [("live", "sk_live_fixture", true), ("test", "sk_test_fixture", true),
            ("sandbox", "sk_test_fixture", true), ("live", "sk_test_fixture", false),
            ("test", "rk_test_fixture", false), ("", "sk_test_fixture", false)] {
            let result = config_with(|name| match name { "ROOTCX_STRIPE_CLIENT_ID" => Some("app-fixture".into()),
                "ROOTCX_STRIPE_CLIENT_SECRET" => Some(key.into()), "ROOTCX_STRIPE_OAUTH_MODE" => Some(mode.into()), _ => None });
            assert_eq!(result.is_ok(), valid, "mode={mode}");
        }
    }
    #[test]
    fn install_link_never_contains_publisher_secret_or_connect_parameters() {
        let result = start(&json!({"clientId":"app-fixture","clientSecret":"must-stay-private"}), "https://app.example/callback", "state-fixture");
        let url = reqwest::Url::parse(result["url"].as_str().unwrap()).unwrap();
        assert_eq!(url.origin().ascii_serialization(), "https://marketplace.stripe.com");
        assert_eq!(url.path(), "/oauth/v2/authorize");
        assert_eq!(url.query_pairs().count(), 3);
        assert!(!url.as_str().contains("must-stay-private"));
    }
    #[test]
    fn external_test_link_is_an_operator_owned_identifier_not_an_arbitrary_url() {
        let build = |mode: &str, link: &str| config_with(|name| match name {
            "ROOTCX_STRIPE_CLIENT_ID" => Some("ca_fixture".into()),
            "ROOTCX_STRIPE_CLIENT_SECRET" => Some(if mode == "live" { "sk_live_fixture" } else { "sk_test_fixture" }.into()),
            "ROOTCX_STRIPE_OAUTH_MODE" => Some(mode.into()),
            "ROOTCX_STRIPE_EXTERNAL_TEST_LINK_ID" => Some(link.into()), _ => None,
        });
        for mode in ["test", "sandbox"] {
            let config = build(mode, "chnlink_AbC123").unwrap();
            let result = start(&config, "https://app.example/callback?value=one&two=three", "state&client_id=attacker");
            let url = reqwest::Url::parse(result["url"].as_str().unwrap()).unwrap();
            assert_eq!(url.origin().ascii_serialization(), "https://marketplace.stripe.com");
            assert_eq!(url.path(), "/oauth/v2/chnlink_AbC123/authorize");
            let query: std::collections::HashMap<_,_> = url.query_pairs().into_owned().collect();
            assert_eq!(query.len(), 3);
            assert_eq!(query["client_id"], "ca_fixture");
            assert_eq!(query["state"], "state&client_id=attacker");
            assert_eq!(query["redirect_uri"], "https://app.example/callback?value=one&two=three");
            assert!(!url.as_str().contains("sk_test_"));
        }
        assert!(build("live", "chnlink_AbC123").is_err());
        for link in ["", "chnlink_", "https://attacker.invalid", "chnlink_x/authorize", "chnlink_..", "chnlink_x?client_id=other", "chnlink_x#fragment", "chnlink_x%2fescape", "chnlink_x\\escape", "chnlink_x\n", "chnlink_é"] {
            assert!(build("sandbox", link).is_err(), "unsafe link identifier");
        }
        assert!(build("sandbox", &format!("chnlink_{}", "a".repeat(129))).is_err());
    }
    #[test]
    fn grants_require_account_scope_mode_and_rotation_without_inventing_response_expiry() {
        let config = json!({"stripeLivemode":false});
        let token = json!({"access_token":"access","refresh_token":"refresh","stripe_user_id":"acct_fixture","livemode":false,"scope":"stripe_apps","token_type":"bearer"});
        let result = credentials(&config, &token).unwrap();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        assert!(result["_expiresAt"].as_u64().unwrap().abs_diff(now + 3600) <= 1);
        for field in ["access_token","refresh_token","stripe_user_id","livemode","scope","token_type"] {
            let mut bad = token.clone(); bad.as_object_mut().unwrap().remove(field);
            assert!(credentials(&config,&bad).is_err(), "missing {field}");
        }
        let mut wrong_mode = token; wrong_mode["livemode"] = json!(true);
        assert!(credentials(&config,&wrong_mode).is_err());
    }
    #[test]
    fn token_exchanges_use_publisher_key_as_basic_username_not_client_id_or_form_secret() {
        use base64::Engine;
        let config = json!({"clientId":"app_fixture", "clientSecret":"sk_test_publisher"});
        for (grant, key) in [("authorization_code", "code"), ("refresh_token", "refresh_token")] {
            let request = token_request(&config, grant, key, "opaque-grant").unwrap().build().unwrap();
            assert_eq!(request.url().as_str(), TOKEN_URL);
            assert_eq!(request.headers()["authorization"], format!("Basic {}", base64::engine::general_purpose::STANDARD.encode("sk_test_publisher:")));
            let body = std::str::from_utf8(request.body().unwrap().as_bytes().unwrap()).unwrap();
            assert_eq!(body, format!("grant_type={grant}&{key}=opaque-grant"));
        }
    }
    #[test]
    fn rotation_rejects_changed_account_or_missing_refresh_and_retains_documented_expiry() {
        let config = json!({"stripeLivemode":false});
        let token = json!({"access_token":"new-access", "refresh_token":"rotated", "stripe_user_id":"acct_original", "livemode":false, "scope":"stripe_apps", "token_type":"bearer"});
        let grant = refreshed_credentials(&config, &token, Some("acct_original")).unwrap();
        assert_eq!(grant["refreshToken"], "rotated");
        for account in [None, Some("acct_other")] { assert!(refreshed_credentials(&config, &token, account).is_err()); }
        let mut missing = token; missing.as_object_mut().unwrap().remove("refresh_token");
        assert!(refreshed_credentials(&config, &missing, Some("acct_original")).is_err());
    }
    #[tokio::test]
    async fn refresh_rejects_tenant_selected_endpoints_before_sending_credentials() {
        for endpoint in ["https://attacker.invalid/token", "https://api.stripe.com/v1/oauth/token?redirect=x", "http://api.stripe.com/v1/oauth/token"] {
            assert!(refresh(&json!({"tokenUrl":endpoint,"tokenEncoding":"form","clientSecret":"fixture"}), "fixture", Some("acct_fixture")).await.is_err());
        }
    }

    #[test]
    fn only_revoked_grants_are_terminal_not_publisher_setup_or_outages() {
        use super::super::super::oauth::RenewError;
        for (status, error, terminal) in [(400, "invalid_grant", true), (401, "invalid_grant", true),
            (401, "invalid_client", false), (429, "invalid_grant", false), (503, "invalid_grant", false)] {
            let result = interpret_refresh(status, &json!({}), &json!({"error":error,"access_token":"never-logged"}), Some("acct_fixture"));
            assert_eq!(matches!(result, Err(RenewError::Rejected(_))), terminal, "{status}/{error}");
            assert!(!result.err().unwrap().message().contains("never-logged"));
        }
    }

}
