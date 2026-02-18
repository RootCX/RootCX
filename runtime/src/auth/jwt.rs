use jsonwebtoken::{Header, Validation};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::AuthConfig;
use crate::RuntimeError;

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    #[serde(default)]
    pub username: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<Uuid>,
    pub exp: i64,
    pub iat: i64,
}

fn encode(config: &AuthConfig, claims: &Claims) -> Result<String, RuntimeError> {
    jsonwebtoken::encode(&Header::default(), claims, &config.encoding_key)
        .map_err(|e| RuntimeError::Auth(e.to_string()))
}

pub fn encode_access(config: &AuthConfig, user_id: Uuid, username: &str) -> Result<String, RuntimeError> {
    let now = chrono::Utc::now().timestamp();
    encode(config, &Claims {
        sub: user_id.to_string(),
        username: username.to_string(),
        session_id: None,
        exp: now + config.access_ttl.as_secs() as i64,
        iat: now,
    })
}

pub fn encode_refresh(config: &AuthConfig, user_id: Uuid, session_id: Uuid) -> Result<String, RuntimeError> {
    let now = chrono::Utc::now().timestamp();
    encode(config, &Claims {
        sub: user_id.to_string(),
        username: String::new(),
        session_id: Some(session_id),
        exp: now + config.refresh_ttl.as_secs() as i64,
        iat: now,
    })
}

pub fn decode(config: &AuthConfig, token: &str) -> Result<Claims, RuntimeError> {
    jsonwebtoken::decode::<Claims>(token, &config.decoding_key, &Validation::default())
        .map(|d| d.claims)
        .map_err(|e| RuntimeError::Auth(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use jsonwebtoken::{EncodingKey, DecodingKey};

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
        let token = encode_access(&config, uid, "alice").unwrap();
        let claims = decode(&config, &token).unwrap();
        assert_eq!(claims.sub, uid.to_string());
        assert_eq!(claims.username, "alice");
        assert!(claims.session_id.is_none());
    }

    #[test]
    fn refresh_token_roundtrip() {
        let config = test_config();
        let uid = Uuid::new_v4();
        let sid = Uuid::new_v4();
        let token = encode_refresh(&config, uid, sid).unwrap();
        let claims = decode(&config, &token).unwrap();
        assert_eq!(claims.sub, uid.to_string());
        assert_eq!(claims.session_id, Some(sid));
    }

    #[test]
    fn decode_invalid_token_fails() {
        assert!(decode(&test_config(), "not-a-jwt").is_err());
    }
}
