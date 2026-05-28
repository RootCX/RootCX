use jsonwebtoken::{Header, Validation};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::AuthConfig;
use crate::RuntimeError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActorClaim {
    pub sub: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    #[serde(default)]
    pub email: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub act: Option<ActorClaim>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aud: Option<String>,
    pub exp: i64,
    pub iat: i64,
}

fn encode(config: &AuthConfig, claims: &Claims) -> Result<String, RuntimeError> {
    jsonwebtoken::encode(&Header::default(), claims, &config.encoding_key)
        .map_err(|e| RuntimeError::Auth(e.to_string()))
}

pub fn encode_access(config: &AuthConfig, user_id: Uuid, email: &str) -> Result<String, RuntimeError> {
    let now = chrono::Utc::now().timestamp();
    encode(
        config,
        &Claims {
            sub: user_id.to_string(),
            email: email.to_string(),
            session_id: None,
            act: None,
            aud: None,
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
            act: None,
            aud: None,
            exp: now + config.refresh_ttl.as_secs() as i64,
            iat: now,
        },
    )
}

pub fn mint_delegated(config: &AuthConfig, delegator_uid: Uuid, agent_uid: Uuid) -> Result<String, RuntimeError> {
    let now = chrono::Utc::now().timestamp();
    let claims = Claims {
        sub: delegator_uid.to_string(),
        email: String::new(),
        session_id: None,
        act: Some(ActorClaim { sub: agent_uid.to_string() }),
        aud: Some("rootcx-core".into()),
        exp: now + 120,
        iat: now,
    };
    jsonwebtoken::encode(&Header::default(), &claims, &config.encoding_key)
        .map_err(|e| RuntimeError::Auth(e.to_string()))
}

pub fn decode(config: &AuthConfig, token: &str) -> Result<Claims, RuntimeError> {
    let mut validation = Validation::new(jsonwebtoken::Algorithm::HS256);
    validation.validate_exp = true;
    validation.validate_aud = false;
    let claims = jsonwebtoken::decode::<Claims>(token, &config.decoding_key, &validation)
        .map(|d| d.claims)
        .map_err(|e| RuntimeError::Auth(e.to_string()))?;
    // Delegated tokens must have the correct audience
    if claims.act.is_some() && claims.aud.as_deref() != Some("rootcx-core") {
        return Err(RuntimeError::Auth("invalid audience for delegated token".into()));
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
        let claims = decode(&config, &token).unwrap();
        assert_eq!(claims.sub, uid.to_string());
        assert_eq!(claims.email, "alice@test.com");
        assert!(claims.session_id.is_none());
        assert!(claims.act.is_none());
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

    #[test]
    fn delegated_token_roundtrip() {
        let config = test_config();
        let delegator = Uuid::new_v4();
        let agent = Uuid::new_v4();
        let token = mint_delegated(&config, delegator, agent).unwrap();
        let claims = decode(&config, &token).unwrap();
        assert_eq!(claims.sub, delegator.to_string());
        let act = claims.act.unwrap();
        assert_eq!(act.sub, agent.to_string());
        assert_eq!(claims.aud.as_deref(), Some("rootcx-core"));
        assert!(claims.exp - claims.iat <= 120);
    }

    #[test]
    fn delegated_token_wrong_aud_rejected() {
        let config = test_config();
        let now = chrono::Utc::now().timestamp();
        let claims = Claims {
            sub: "user-a".into(),
            email: String::new(),
            session_id: None,
            act: Some(ActorClaim { sub: "agent-b".into() }),
            aud: Some("wrong-audience".into()),
            exp: now + 120,
            iat: now,
        };
        let token = jsonwebtoken::encode(&Header::default(), &claims, &config.encoding_key).unwrap();
        assert!(decode(&config, &token).is_err(), "delegated token with wrong aud must be rejected");
    }

    #[test]
    fn legacy_token_without_act_decodes() {
        let config = test_config();
        let uid = Uuid::new_v4();
        let token = encode_access(&config, uid, "bob@test.com").unwrap();
        let claims = decode(&config, &token).unwrap();
        assert!(claims.act.is_none());
        assert!(claims.aud.is_none());
    }
}
