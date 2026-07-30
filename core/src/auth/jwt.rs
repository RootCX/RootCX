use jsonwebtoken::{Header, Validation};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::AuthConfig;
use crate::RuntimeError;

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    #[serde(default)]
    pub email: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aud: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    pub exp: i64,
    pub iat: i64,
}

fn encode(config: &AuthConfig, claims: &Claims) -> Result<String, RuntimeError> {
    jsonwebtoken::encode(&Header::default(), claims, &config.encoding_key)
        .map_err(|e| RuntimeError::Auth(e.to_string()))
}

fn decode_with_validation(
    config: &AuthConfig,
    token: &str,
    validation: &Validation,
) -> Result<Claims, RuntimeError> {
    jsonwebtoken::decode::<Claims>(token, &config.decoding_key, validation)
        .map(|data| data.claims)
        .map_err(|error| RuntimeError::Auth(error.to_string()))
}

pub fn encode_access(config: &AuthConfig, user_id: Uuid, email: &str) -> Result<String, RuntimeError> {
    let now = chrono::Utc::now().timestamp();
    encode(
        config,
        &Claims {
            sub: user_id.to_string(),
            email: email.to_string(),
            session_id: None,
            aud: None,
            scope: None,
            exp: now + config.access_ttl.as_secs() as i64,
            iat: now,
        },
    )
}

pub fn encode_access_for_audience(
    config: &AuthConfig,
    user_id: Uuid,
    email: &str,
    audience: &str,
    scope: &str,
) -> Result<String, RuntimeError> {
    let now = chrono::Utc::now().timestamp();
    encode(
        config,
        &Claims {
            sub: user_id.to_string(),
            email: email.to_string(),
            session_id: None,
            aud: Some(audience.to_string()),
            scope: Some(scope.to_string()),
            exp: now + config.access_ttl.as_secs() as i64,
            iat: now,
        },
    )
}

pub fn encode_refresh(config: &AuthConfig, user_id: Uuid, session_id: Uuid) -> Result<String, RuntimeError> {
    let now = chrono::Utc::now().timestamp();
    encode(
        config,
        &Claims {
            sub: user_id.to_string(),
            email: String::new(),
            session_id: Some(session_id),
            aud: None,
            scope: None,
            exp: now + config.refresh_ttl.as_secs() as i64,
            iat: now,
        },
    )
}

pub fn decode_for_audience(config: &AuthConfig, token: &str, audience: &str) -> Result<Claims, RuntimeError> {
    let mut validation = Validation::new(jsonwebtoken::Algorithm::HS256);
    validation.validate_exp = true;
    validation.set_audience(&[audience]);
    let claims = decode_with_validation(config, token, &validation)?;
    if claims.aud.as_deref() != Some(audience) {
        return Err(RuntimeError::Auth("missing or invalid token audience".into()));
    }
    Ok(claims)
}

/// Decode a RootCX token that is not delegated to a specific resource.
///
/// Audience-bound tokens (such as MCP OAuth access tokens) must only be
/// decoded through `decode_for_audience`. Rejecting both `aud` and `scope`
/// here prevents a delegated token from being replayed against REST routes.
pub fn decode_unscoped(config: &AuthConfig, token: &str) -> Result<Claims, RuntimeError> {
    let mut validation = Validation::new(jsonwebtoken::Algorithm::HS256);
    validation.validate_exp = true;
    validation.validate_aud = false;
    let claims = decode_with_validation(config, token, &validation)?;
    if claims.aud.is_some() || claims.scope.is_some() {
        return Err(RuntimeError::Auth(
            "resource-scoped token requires audience validation".into(),
        ));
    }
    Ok(claims)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{DecodingKey, EncodingKey};
    use std::time::Duration;

    fn test_config() -> AuthConfig {
        let secret = b"test-secret-key-for-unit-tests!!";
        AuthConfig {
            encoding_key: EncodingKey::from_secret(secret),
            decoding_key: DecodingKey::from_secret(secret),
            access_ttl: Duration::from_secs(900),
            refresh_ttl: Duration::from_secs(86400),
        }
    }

    #[test]
    fn access_token_roundtrip() {
        let config = test_config();
        let uid = Uuid::new_v4();
        let token = encode_access(&config, uid, "alice@test.com").unwrap();
        let claims = decode_unscoped(&config, &token).unwrap();
        assert_eq!(claims.sub, uid.to_string());
        assert_eq!(claims.email, "alice@test.com");
        assert!(claims.session_id.is_none());
    }

    #[test]
    fn refresh_token_roundtrip() {
        let config = test_config();
        let uid = Uuid::new_v4();
        let sid = Uuid::new_v4();
        let token = encode_refresh(&config, uid, sid).unwrap();
        let claims = decode_unscoped(&config, &token).unwrap();
        assert_eq!(claims.sub, uid.to_string());
        assert_eq!(claims.session_id, Some(sid));
    }

    #[test]
    fn decode_invalid_token_fails() {
        assert!(decode_unscoped(&test_config(), "not-a-jwt").is_err());
    }

    #[test]
    fn audience_token_is_bound_to_its_mcp_resource() {
        let config = test_config();
        let uid = Uuid::new_v4();
        let expected = "https://tenant.rootcx.com/mcp";
        let token = encode_access_for_audience(
            &config,
            uid,
            "alice@test.com",
            expected,
            "mcp:read",
        )
        .unwrap();
        let claims = decode_for_audience(&config, &token, expected).unwrap();
        assert_eq!(claims.scope.as_deref(), Some("mcp:read"));
        assert!(decode_for_audience(&config, &token, "https://other.rootcx.com/mcp").is_err());
        assert!(decode_unscoped(&config, &token).is_err());

        let unscoped = encode_access(&config, uid, "alice@test.com").unwrap();
        assert!(decode_for_audience(&config, &unscoped, expected).is_err());
    }
}
