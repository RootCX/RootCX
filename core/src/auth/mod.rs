pub mod identity;
pub mod jwt;
pub mod password;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use jsonwebtoken::{DecodingKey, EncodingKey};
use tracing::{info, warn};

use crate::RuntimeError;

pub struct AuthConfig {
    pub encoding_key: EncodingKey,
    pub decoding_key: DecodingKey,
    pub access_ttl: Duration,
    pub refresh_ttl: Duration,
    // anonymous access when true; tokens still validated if provided
    pub public: bool,
}

impl AuthConfig {
    pub fn load(data_dir: &Path, auth_required: Option<bool>) -> Result<Arc<Self>, RuntimeError> {
        let public = match auth_required {
            Some(required) => !required,
            None => std::env::var("ROOTCX_AUTH").map(|v| v == "public").unwrap_or(false),
        };

        if public {
            warn!("auth mode: public (set ROOTCX_AUTH=required or remove ROOTCX_AUTH=public to enforce)");
        } else {
            info!("auth mode: required");
        }

        let secret = if let Ok(s) = std::env::var("ROOTCX_JWT_SECRET") {
            if s.len() < 32 {
                return Err(RuntimeError::Auth("ROOTCX_JWT_SECRET must be at least 32 bytes".into()));
            }
            info!("JWT secret loaded from env");
            s.into_bytes()
        } else {
            load_or_generate_jwt_key(&data_dir.join("config/jwt.key"))?
        };

        Ok(Arc::new(Self {
            encoding_key: EncodingKey::from_secret(&secret),
            decoding_key: DecodingKey::from_secret(&secret),
            access_ttl: Duration::from_secs(15 * 60),         // 15 min
            refresh_ttl: Duration::from_secs(30 * 24 * 3600), // 30 days
            public,
        }))
    }
}

fn load_or_generate_jwt_key(path: &Path) -> Result<Vec<u8>, RuntimeError> {
    fn err(msg: impl std::fmt::Display) -> RuntimeError {
        RuntimeError::Auth(msg.to_string())
    }

    if path.exists() {
        let content = std::fs::read_to_string(path).map_err(err)?;
        let bytes = hex::decode(content.trim()).map_err(err)?;
        info!(path = %path.display(), "JWT key loaded from file");
        return Ok(bytes);
    }

    warn!("no JWT key found, generating");
    let mut key = vec![0u8; 32];
    use rand::RngCore;
    rand::rngs::OsRng.fill_bytes(&mut key);

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(err)?;
    }
    std::fs::write(path, hex::encode(&key)).map_err(err)?;

    let _ = rootcx_platform::fs::set_private(path);

    info!(path = %path.display(), "JWT key generated");
    Ok(key)
}
