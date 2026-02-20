use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Arc;

use reqwest::{multipart, Client, Response, StatusCode};
use rootcx_postgres_mgmt::PostgresManager;
use rootcx_runtime::{server, Runtime};
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::sync::Mutex;

pub struct TestRuntime {
    base_url: String,
    pub pg_port: u16,
    client: Client,
    runtime: Arc<Mutex<Runtime>>,
    token: String,
    _tmp: TempDir,
}

impl TestRuntime {
    pub async fn boot() -> Self {
        Self::boot_inner(false).await
    }

    pub async fn boot_with_auth() -> Self {
        Self::boot_inner(true).await
    }

    async fn boot_inner(require_auth: bool) -> Self {
        let pg_root = find_pg_root(&PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources"))
            .expect("bundled PostgreSQL not found");
        let tmp = TempDir::new().unwrap();
        let data_dir = tmp.path().to_path_buf();
        let pg_port = free_port();
        let api_port = free_port();

        let pg = PostgresManager::new(pg_root.join("bin"), data_dir.join("data/pg"), pg_port)
            .with_lib_dir(pg_root.join("lib"));
        let auth_required = if require_auth { Some(true) } else { None };
        let runtime = Arc::new(Mutex::new(Runtime::with_auth_mode(pg, data_dir, auth_required)));
        runtime.lock().await.boot(api_port).await.expect("boot failed");

        let rt = Arc::clone(&runtime);
        tokio::spawn(async move { server::serve(rt, api_port).await.ok(); });

        let base_url = format!("http://127.0.0.1:{api_port}");
        let client = Client::new();
        let health = format!("{base_url}/health");
        for _ in 0..100 {
            if client.get(&health).send().await.is_ok() { break; }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }

        let token = if require_auth {
            let creds = json!({"username":"testadmin","password":"testpass123"});
            client.post(format!("{base_url}/api/v1/auth/register")).json(&creds).send().await.ok();
            let res = client.post(format!("{base_url}/api/v1/auth/login")).json(&creds).send().await.unwrap();
            let body: Value = res.json().await.unwrap();
            body["accessToken"].as_str().unwrap().to_string()
        } else {
            String::new()
        };

        Self { base_url, pg_port, client, runtime, token, _tmp: tmp }
    }

    pub fn url(&self, path: &str) -> String { format!("{}{path}", self.base_url) }

    fn maybe_auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if self.token.is_empty() { req } else { req.bearer_auth(&self.token) }
    }

    pub async fn get(&self, path: &str) -> Response {
        self.maybe_auth(self.client.get(self.url(path))).send().await.unwrap()
    }

    pub async fn get_json(&self, path: &str) -> (StatusCode, Value) {
        let r = self.get(path).await;
        let s = r.status();
        (s, r.json().await.unwrap_or(Value::Null))
    }

    pub async fn post_json(&self, path: &str, body: &Value) -> (StatusCode, Value) {
        let r = self.maybe_auth(self.client.post(self.url(path))).json(body).send().await.unwrap();
        let s = r.status();
        (s, r.json().await.unwrap_or(Value::Null))
    }

    pub async fn patch_json(&self, path: &str, body: &Value) -> (StatusCode, Value) {
        let r = self.maybe_auth(self.client.patch(self.url(path))).json(body).send().await.unwrap();
        let s = r.status();
        (s, r.json().await.unwrap_or(Value::Null))
    }

    pub async fn delete(&self, path: &str) -> StatusCode {
        self.maybe_auth(self.client.delete(self.url(path))).send().await.unwrap().status()
    }

    pub async fn upload(&self, path: &str, name: &str, mime: &str, data: &[u8]) -> (StatusCode, Value) {
        let part = multipart::Part::bytes(data.to_vec()).file_name(name.to_string()).mime_str(mime).unwrap();
        let form = multipart::Form::new().part("file", part);
        let r = self.maybe_auth(self.client.post(self.url(path))).multipart(form).send().await.unwrap();
        let s = r.status();
        (s, r.json().await.unwrap_or(Value::Null))
    }

    pub async fn get_unauthed(&self, path: &str) -> StatusCode {
        self.client.get(self.url(path)).send().await.unwrap().status()
    }

    pub async fn post_json_unauthed(&self, path: &str, body: &Value) -> StatusCode {
        self.client.post(self.url(path)).json(body).send().await.unwrap().status()
    }

    pub async fn delete_unauthed(&self, path: &str) -> StatusCode {
        self.client.delete(self.url(path)).send().await.unwrap().status()
    }

    pub async fn install(&self, app_id: &str, entity: &str) {
        let manifest = json!({
            "appId": app_id, "name": app_id, "version": "1.0.0",
            "dataContract": [{ "entityName": entity, "fields": [
                { "name": "first_name", "type": "text", "required": true },
                { "name": "last_name",  "type": "text", "required": true },
                { "name": "email", "type": "text" },
                { "name": "phone", "type": "text" },
                { "name": "company", "type": "text" },
                { "name": "notes", "type": "text" },
            ]}]
        });
        let (s, _) = self.post_json("/api/v1/apps", &manifest).await;
        assert_eq!(s, 200, "install {app_id} failed");
    }

    pub async fn install_manifest(&self, manifest: &Value) {
        let (s, _) = self.post_json("/api/v1/apps", manifest).await;
        assert_eq!(s, 200, "install_manifest failed");
    }

    pub async fn create(&self, app: &str, entity: &str, body: &Value) -> Value {
        let (s, v) = self.post_json(&format!("/api/v1/apps/{app}/collections/{entity}"), body).await;
        assert_eq!(s, 201);
        v
    }

    pub async fn deploy(&self, app_id: &str, data: &[u8]) -> (StatusCode, Value) {
        self.upload(&format!("/api/v1/apps/{app_id}/deploy"), "backend.tar.gz", "application/gzip", data).await
    }

    pub async fn shutdown(self) {
        self.runtime.lock().await.shutdown().await.expect("shutdown failed");
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn find_pg_root(dir: &std::path::Path) -> Option<PathBuf> {
    std::fs::read_dir(dir).ok()?.flatten().find_map(|e| {
        let p = e.path();
        (p.is_dir() && p.join("bin/pg_ctl").exists()).then_some(p)
    })
}
