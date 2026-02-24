use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::{broadcast, oneshot, Mutex};

const CMD_TIMEOUT: Duration = Duration::from_secs(30);
const BROADCAST_CAPACITY: usize = 64;

pub struct BrowserQueue {
    pending: Mutex<HashMap<u64, oneshot::Sender<Value>>>,
    cmd_tx: broadcast::Sender<String>,
    next_id: AtomicU64,
}

impl BrowserQueue {
    pub fn new() -> Self {
        let (cmd_tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            pending: Mutex::new(HashMap::new()),
            cmd_tx,
            next_id: AtomicU64::new(1),
        }
    }

    /// Queue a browser command and wait for the studio to execute it.
    pub async fn submit(&self, action: &str, params: Value) -> Result<Value, String> {
        if self.cmd_tx.receiver_count() == 0 {
            return Err("no studio connected — open the Studio app to enable browsing".into());
        }

        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();

        self.pending.lock().await.insert(id, tx);

        let msg = json!({ "id": id, "action": action, "params": params }).to_string();
        if self.cmd_tx.send(msg).is_err() {
            self.pending.lock().await.remove(&id);
            return Err("failed to send command to studio".into());
        }

        match tokio::time::timeout(CMD_TIMEOUT, rx).await {
            Ok(Ok(val)) => {
                if let Some(err) = val.get("error").and_then(|e| e.as_str()) {
                    Err(err.to_string())
                } else {
                    Ok(val)
                }
            }
            Ok(Err(_)) => Err("studio dropped the command".into()),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                Err(format!("browser command timed out after {}s", CMD_TIMEOUT.as_secs()))
            }
        }
    }

    /// Called when the studio POSTs a result back.
    pub async fn resolve(&self, cmd_id: u64, result: Value) {
        if let Some(tx) = self.pending.lock().await.remove(&cmd_id) {
            let _ = tx.send(result);
        }
    }

    /// Subscribe to the command stream (for SSE endpoint).
    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.cmd_tx.subscribe()
    }
}
