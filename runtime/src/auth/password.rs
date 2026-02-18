use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};

use crate::RuntimeError;

pub fn hash(password: &str) -> Result<String, RuntimeError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| RuntimeError::Auth(format!("password hash: {e}")))
}

pub fn verify(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash)
        .ok()
        .map(|parsed| Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_and_verify() {
        let h = hash("test1234").unwrap();
        assert!(verify("test1234", &h));
        assert!(!verify("wrong", &h));
    }

    #[test]
    fn verify_bad_hash_returns_false() {
        assert!(!verify("anything", "not-a-valid-hash"));
    }
}
