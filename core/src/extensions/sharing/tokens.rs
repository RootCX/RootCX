use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use rand::RngCore;
use rand::rngs::OsRng;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// Length of the raw token after base64url encoding (32 bytes → 43 chars, no padding).
pub const TOKEN_LEN: usize = 43;

/// Length of the SHA-256 digest used for DB storage and comparison.
pub const HASH_LEN: usize = 32;

/// Generate a fresh share token. 32 bytes from OsRng, base64url-encoded.
///
/// 256 bits of entropy — infeasible to brute force at any practical scale.
pub fn generate() -> String {
    let mut buf = [0u8; 32];
    OsRng.fill_bytes(&mut buf);
    B64.encode(buf)
}

/// SHA-256 hash of the raw token. This is what we store in the DB.
///
/// We don't use argon2/bcrypt: the token is already CSPRNG-generated with 256 bits
/// of entropy, so the security comes from unguessability, not from slow hashing.
pub fn hash(raw: &str) -> [u8; HASH_LEN] {
    let mut h = Sha256::new();
    h.update(raw.as_bytes());
    h.finalize().into()
}

/// Constant-time comparison between two hashes. Returns false on length mismatch.
pub fn verify(expected: &[u8], candidate: &[u8]) -> bool {
    if expected.len() != candidate.len() {
        return false;
    }
    expected.ct_eq(candidate).into()
}

/// First 8 chars of the raw token — safe to store in plain for owner-UI display.
pub fn prefix(raw: &str) -> String {
    raw.chars().take(8).collect()
}

/// Cheap pre-validation: reject obvious garbage before hitting the DB.
pub fn is_well_formed(raw: &str) -> bool {
    raw.len() == TOKEN_LEN
        && raw.bytes().all(|b| {
            b.is_ascii_alphanumeric() || b == b'-' || b == b'_'
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_is_43_chars_base64url() {
        let t = generate();
        assert_eq!(t.len(), TOKEN_LEN);
        assert!(is_well_formed(&t));
    }

    #[test]
    fn generate_returns_distinct_tokens() {
        let a = generate();
        let b = generate();
        assert_ne!(a, b);
    }

    #[test]
    fn hash_is_deterministic() {
        let t = "fixed-token-for-hash-test-aaaaaaaaaaaaaaaaaa";
        assert_eq!(hash(t), hash(t));
    }

    #[test]
    fn hash_differs_for_different_input() {
        assert_ne!(hash("token-a"), hash("token-b"));
    }

    #[test]
    fn verify_constant_time_compare() {
        let a = hash("same");
        let b = hash("same");
        assert!(verify(&a, &b));
        assert!(!verify(&a, &hash("different")));
    }

    #[test]
    fn verify_rejects_length_mismatch() {
        let a = hash("token");
        assert!(!verify(&a, &a[..16]));
        assert!(!verify(&a[..16], &a));
    }

    #[test]
    fn is_well_formed_rejects_bad_input() {
        for (label, input) in [
            ("empty", "".to_string()),
            ("short", "short".to_string()),
            ("too long", "a".repeat(TOKEN_LEN + 1)),
            ("contains dot", format!("{}.", "a".repeat(TOKEN_LEN - 1))),
            ("contains space", format!("{} ", "a".repeat(TOKEN_LEN - 1))),
            ("contains slash", format!("{}/", "a".repeat(TOKEN_LEN - 1))),
            ("contains plus", format!("{}+", "a".repeat(TOKEN_LEN - 1))),
            ("contains equals", format!("{}=", "a".repeat(TOKEN_LEN - 1))),
        ] {
            assert!(!is_well_formed(&input), "should reject: {label}");
        }
    }

    #[test]
    fn is_well_formed_rejects_jwt_format() {
        // JWTs always contain dots — this is the security-critical discriminator
        // that prevents a JWT from being treated as a share token in CallerAuth.
        let fake_jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.signature";
        assert!(!is_well_formed(fake_jwt));
        // Even a 43-char string with a dot must be rejected
        let dot_at_21 = format!("{}.{}", "a".repeat(21), "b".repeat(21));
        assert!(!is_well_formed(&dot_at_21));
    }

    #[test]
    fn verify_rejects_single_bit_flip() {
        let h = hash("real-token");
        let mut flipped = h;
        flipped[0] ^= 1; // flip one bit
        assert!(!verify(&h, &flipped));
    }

    #[test]
    fn prefix_takes_first_8_chars() {
        assert_eq!(prefix("ABCDEFGHIJKLMNOP"), "ABCDEFGH");
    }
}
