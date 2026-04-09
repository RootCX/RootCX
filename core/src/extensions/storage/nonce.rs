use std::collections::HashMap;
use std::time::{Duration, Instant};

const NONCE_TTL: Duration = Duration::from_secs(60);

pub struct UploadNonce {
    pub app_id: String,
    pub name: String,
    pub content_type: String,
    pub max_size: usize,
    pub expires_at: Instant,
}

#[derive(Default)]
pub struct NonceStore {
    nonces: HashMap<String, UploadNonce>,
}

impl NonceStore {
    pub fn create(&mut self, app_id: &str, name: &str, content_type: &str, max_size: usize) -> String {
        self.cleanup_expired();
        let nonce = uuid::Uuid::new_v4().to_string();
        self.nonces.insert(nonce.clone(), UploadNonce {
            app_id: app_id.to_string(),
            name: name.to_string(),
            content_type: content_type.to_string(),
            max_size,
            expires_at: Instant::now() + NONCE_TTL,
        });
        nonce
    }

    /// Consume a nonce — returns it if valid, removes it from the store.
    pub fn consume(&mut self, nonce: &str) -> Option<UploadNonce> {
        let entry = self.nonces.remove(nonce)?;
        if Instant::now() > entry.expires_at {
            return None; // expired
        }
        Some(entry)
    }

    fn cleanup_expired(&mut self) {
        let now = Instant::now();
        self.nonces.retain(|_, v| v.expires_at > now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_and_consume() {
        let mut store = NonceStore::default();
        let nonce = store.create("peppol", "test.xml", "application/xml", 1024);
        let entry = store.consume(&nonce).expect("nonce should be valid");
        assert_eq!(entry.app_id, "peppol");
        assert_eq!(entry.name, "test.xml");
    }

    #[test]
    fn consume_is_single_use() {
        let mut store = NonceStore::default();
        let nonce = store.create("peppol", "test.xml", "application/xml", 1024);
        assert!(store.consume(&nonce).is_some());
        assert!(store.consume(&nonce).is_none()); // second use fails
    }

    #[test]
    fn unknown_nonce_returns_none() {
        let mut store = NonceStore::default();
        assert!(store.consume("nonexistent").is_none());
    }
}
