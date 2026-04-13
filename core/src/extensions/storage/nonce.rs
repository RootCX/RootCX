use std::collections::HashMap;
use std::time::{Duration, Instant};

const NONCE_TTL: Duration = Duration::from_secs(60);
const DOWNLOAD_NONCE_TTL: Duration = Duration::from_secs(300);

pub struct UploadNonce {
    pub app_id: String,
    pub name: String,
    pub content_type: String,
    pub max_size: usize,
    pub expires_at: Instant,
}

pub struct DownloadNonce {
    pub file_id: uuid::Uuid,
    pub app_id: String,
    pub expires_at: Instant,
}

#[derive(Default)]
pub struct NonceStore {
    nonces: HashMap<String, UploadNonce>,
    download_nonces: HashMap<String, DownloadNonce>,
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

    pub fn create_download(&mut self, file_id: uuid::Uuid, app_id: &str) -> String {
        self.cleanup_expired();
        let nonce = uuid::Uuid::new_v4().to_string();
        self.download_nonces.insert(nonce.clone(), DownloadNonce {
            file_id,
            app_id: app_id.to_string(),
            expires_at: Instant::now() + DOWNLOAD_NONCE_TTL,
        });
        nonce
    }

    pub fn consume_download(&mut self, nonce: &str) -> Option<DownloadNonce> {
        let entry = self.download_nonces.remove(nonce)?;
        if Instant::now() > entry.expires_at { return None; }
        Some(entry)
    }

    fn cleanup_expired(&mut self) {
        let now = Instant::now();
        self.nonces.retain(|_, v| v.expires_at > now);
        self.download_nonces.retain(|_, v| v.expires_at > now);
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
    fn download_create_and_consume() {
        let mut store = NonceStore::default();
        let file_id = uuid::Uuid::new_v4();
        let nonce = store.create_download(file_id, "my_app");
        let entry = store.consume_download(&nonce).expect("nonce should be valid");
        assert_eq!(entry.file_id, file_id);
        assert_eq!(entry.app_id, "my_app");
    }

    #[test]
    fn download_consume_is_single_use() {
        let mut store = NonceStore::default();
        let nonce = store.create_download(uuid::Uuid::new_v4(), "app");
        assert!(store.consume_download(&nonce).is_some());
        assert!(store.consume_download(&nonce).is_none());
    }

    #[test]
    fn download_unknown_nonce_returns_none() {
        let mut store = NonceStore::default();
        assert!(store.consume_download("nonexistent").is_none());
    }

    #[test]
    fn upload_and_download_nonces_are_independent() {
        // A download nonce must not be consumable via the upload path and vice versa.
        let mut store = NonceStore::default();
        let upload_nonce = store.create("app", "f.csv", "text/csv", 0);
        let download_nonce = store.create_download(uuid::Uuid::new_v4(), "app");
        assert!(store.consume_download(&upload_nonce).is_none());
        assert!(store.consume(&download_nonce).is_none());
        // Originals still work via their correct paths
        assert!(store.consume(&upload_nonce).is_some());
        assert!(store.consume_download(&download_nonce).is_some());
    }

    #[test]
    fn unknown_nonce_returns_none() {
        let mut store = NonceStore::default();
        assert!(store.consume("nonexistent").is_none());
    }
}
