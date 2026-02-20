use std::path::Path;

use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Nonce};
use rand::RngCore;
use sqlx::PgPool;
use tracing::{info, warn};

use crate::RuntimeError;

const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32;

fn secret_err(msg: impl std::fmt::Display) -> RuntimeError {
    RuntimeError::Secret(msg.to_string())
}

pub struct SecretManager {
    cipher: Aes256Gcm,
}

#[cfg(test)]
impl SecretManager {
    fn with_key(key: &[u8; 32]) -> Self {
        Self { cipher: Aes256Gcm::new_from_slice(key).unwrap() }
    }
}

impl SecretManager {
    pub fn new(data_dir: &Path) -> Result<Self, RuntimeError> {
        let key_bytes = if let Ok(hex) = std::env::var("ROOTCX_MASTER_KEY") {
            let bytes = hex::decode(&hex).map_err(secret_err)?;
            if bytes.len() != KEY_LEN {
                return Err(secret_err(format!("ROOTCX_MASTER_KEY must be {KEY_LEN} bytes")));
            }
            info!("master key loaded from env");
            bytes
        } else {
            load_or_generate_key(&data_dir.join("config/master.key"))?
        };

        let cipher = Aes256Gcm::new_from_slice(&key_bytes).map_err(|e| secret_err(format!("AES init failed: {e}")))?;
        Ok(Self { cipher })
    }

    pub fn encrypt(&self, plaintext: &str) -> Result<(Vec<u8>, Vec<u8>), RuntimeError> {
        let mut nonce_bytes = [0u8; NONCE_LEN];
        OsRng.fill_bytes(&mut nonce_bytes);
        let ciphertext = self
            .cipher
            .encrypt(Nonce::from_slice(&nonce_bytes), plaintext.as_bytes())
            .map_err(|e| secret_err(format!("encrypt: {e}")))?;
        Ok((nonce_bytes.to_vec(), ciphertext))
    }

    pub fn decrypt(&self, nonce: &[u8], ciphertext: &[u8]) -> Result<String, RuntimeError> {
        if nonce.len() != NONCE_LEN {
            return Err(secret_err(format!("bad nonce len: {}", nonce.len())));
        }
        let plaintext = self
            .cipher
            .decrypt(Nonce::from_slice(nonce), ciphertext)
            .map_err(|e| secret_err(format!("decrypt: {e}")))?;
        String::from_utf8(plaintext).map_err(secret_err)
    }

    pub async fn set(&self, pool: &PgPool, app_id: &str, key_name: &str, plaintext: &str) -> Result<(), RuntimeError> {
        let (nonce, ciphertext) = self.encrypt(plaintext)?;
        sqlx::query(
            "INSERT INTO rootcx_system.secrets (app_id, key_name, nonce, ciphertext) VALUES ($1, $2, $3, $4)
             ON CONFLICT (app_id, key_name) DO UPDATE SET nonce = EXCLUDED.nonce, ciphertext = EXCLUDED.ciphertext",
        )
        .bind(app_id)
        .bind(key_name)
        .bind(&nonce)
        .bind(&ciphertext)
        .execute(pool)
        .await
        .map_err(secret_err)?;
        info!(app_id, key_name, "secret stored");
        Ok(())
    }

    pub async fn get(&self, pool: &PgPool, app_id: &str, key_name: &str) -> Result<Option<String>, RuntimeError> {
        let row: Option<(Vec<u8>, Vec<u8>)> =
            sqlx::query_as("SELECT nonce, ciphertext FROM rootcx_system.secrets WHERE app_id = $1 AND key_name = $2")
                .bind(app_id)
                .bind(key_name)
                .fetch_optional(pool)
                .await
                .map_err(secret_err)?;

        row.map(|(n, c)| self.decrypt(&n, &c)).transpose()
    }

    pub async fn get_all_for_app(&self, pool: &PgPool, app_id: &str) -> Result<Vec<(String, String)>, RuntimeError> {
        let rows: Vec<(String, Vec<u8>, Vec<u8>)> =
            sqlx::query_as("SELECT key_name, nonce, ciphertext FROM rootcx_system.secrets WHERE app_id = $1")
                .bind(app_id)
                .fetch_all(pool)
                .await
                .map_err(secret_err)?;

        rows.into_iter().map(|(k, n, c)| Ok((k, self.decrypt(&n, &c)?))).collect()
    }

    pub async fn delete(&self, pool: &PgPool, app_id: &str, key_name: &str) -> Result<bool, RuntimeError> {
        let r = sqlx::query("DELETE FROM rootcx_system.secrets WHERE app_id = $1 AND key_name = $2")
            .bind(app_id)
            .bind(key_name)
            .execute(pool)
            .await
            .map_err(secret_err)?;
        Ok(r.rows_affected() > 0)
    }

    pub async fn list_keys(&self, pool: &PgPool, app_id: &str) -> Result<Vec<String>, RuntimeError> {
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT key_name FROM rootcx_system.secrets WHERE app_id = $1 ORDER BY key_name")
                .bind(app_id)
                .fetch_all(pool)
                .await
                .map_err(secret_err)?;
        Ok(rows.into_iter().map(|(k,)| k).collect())
    }
}

pub async fn bootstrap_secrets_schema(pool: &PgPool) -> Result<(), RuntimeError> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS rootcx_system.secrets (
            app_id TEXT NOT NULL, key_name TEXT NOT NULL,
            nonce BYTEA NOT NULL, ciphertext BYTEA NOT NULL,
            PRIMARY KEY (app_id, key_name)
        )",
    )
    .execute(pool)
    .await
    .map_err(RuntimeError::Schema)?;
    info!("secrets schema ready");
    Ok(())
}

fn load_or_generate_key(path: &Path) -> Result<Vec<u8>, RuntimeError> {
    if path.exists() {
        let content = std::fs::read_to_string(path).map_err(secret_err)?;
        let bytes = hex::decode(content.trim()).map_err(secret_err)?;
        if bytes.len() != KEY_LEN {
            return Err(secret_err("master.key: wrong length"));
        }
        info!(path = %path.display(), "master key loaded from file");
        return Ok(bytes);
    }

    warn!("no master key found, generating");
    let mut key = vec![0u8; KEY_LEN];
    OsRng.fill_bytes(&mut key);

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(secret_err)?;
    }
    std::fs::write(path, hex::encode(&key)).map_err(secret_err)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }

    info!(path = %path.display(), "master key generated");
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mgr() -> SecretManager {
        SecretManager::with_key(&[0xAA; 32])
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let m = mgr();
        let (nonce, ciphertext) = m.encrypt("hello").unwrap();
        let decrypted = m.decrypt(&nonce, &ciphertext).unwrap();
        assert_eq!(decrypted, "hello");
    }

    #[test]
    fn decrypt_wrong_nonce_fails() {
        let m = mgr();
        let (mut nonce, ciphertext) = m.encrypt("secret").unwrap();
        nonce[0] ^= 0xFF;
        assert!(m.decrypt(&nonce, &ciphertext).is_err());
    }

    #[test]
    fn decrypt_wrong_ciphertext_fails() {
        let m = mgr();
        let (nonce, mut ciphertext) = m.encrypt("secret").unwrap();
        ciphertext[0] ^= 0xFF;
        assert!(m.decrypt(&nonce, &ciphertext).is_err());
    }

    #[test]
    fn decrypt_bad_nonce_length() {
        let m = mgr();
        let bad_nonce = [0u8; 10];
        let err = m.decrypt(&bad_nonce, &[0u8; 16]).unwrap_err();
        assert!(err.to_string().contains("bad nonce len"));
    }

    #[test]
    fn encrypt_empty_string() {
        let m = mgr();
        let (nonce, ciphertext) = m.encrypt("").unwrap();
        let decrypted = m.decrypt(&nonce, &ciphertext).unwrap();
        assert_eq!(decrypted, "");
    }
}
