// Shared integration-test support. Keeping this module below a directory
// prevents Cargo from compiling it as an integration-test target of its own.
use std::net::TcpListener;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use reqwest::{Client, Method, StatusCode, multipart};
use rootcx_core::{ReadyRuntime, Runtime, server};
use serde_json::{Value, json};
use tempfile::TempDir;
use sqlx::{Connection, PgConnection, postgres::PgConnectOptions};

// This harness is shared by integration targets with intentionally different
// surfaces; each target uses only the helpers it needs.
#[allow(dead_code)]
pub struct TestRuntime {
    base_url: String,
    pub client: Client,
    pub runtime: Arc<ReadyRuntime>,
    pub token: String,
    server: tokio::task::JoinHandle<()>,
    _tmp: TempDir,
    _database: PgConnection,
}

#[allow(dead_code)]
impl TestRuntime {
    pub async fn boot() -> Self {
        Self::boot_with(None).await
    }

    /// Boot with an explicit worker-pool cap, to exercise behaviour at the limit
    /// without provisioning a memory-constrained container.
    pub async fn boot_capped(max_workers: usize) -> Self {
        Self::boot_with(Some(max_workers)).await
    }

    async fn boot_with(max_workers: Option<usize>) -> Self {
        let started = Instant::now();
        timing("before fixture", started);
        // HTTP errors redact SQL details; keep them visible in test output.
        let _ = tracing_subscriber::fmt()
            .with_test_writer()
            .with_max_level(tracing::Level::ERROR)
            .try_init();
        let resources = rootcx_platform::dirs::resources_dir(env!("CARGO_MANIFEST_DIR"))
            .expect("resources dir not found");
        let bun_bin = rootcx_platform::bin::binary_path(&resources, "bun");
        let tmp = TempDir::new().unwrap();
        let data_dir = tmp.path().to_path_buf();
        let api_port = free_port();

        let (database, db_url) = fresh_database().await;
        timing("postgres", started);
        let started = Instant::now();

        let resources_dir = data_dir.join("resources");
        std::fs::create_dir_all(&resources_dir).unwrap();
        let mut runtime = Runtime::new(db_url, data_dir, resources_dir, bun_bin)
            .with_assistant_dir(
                Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/assistant"),
            );
        if let Some(max) = max_workers {
            runtime = runtime.with_max_workers(max);
        }
        let runtime = Arc::new(runtime.boot(api_port).await.expect("boot failed"));
        timing("core boot", started);
        let started = Instant::now();
        let rt = Arc::clone(&runtime);
        let server = tokio::spawn(async move {
            server::serve(rt, api_port).await.ok();
        });

        let base_url = format!("http://127.0.0.1:{api_port}");
        // All harness traffic is loopback. Do not consult host proxy settings.
        let client = Client::builder().no_proxy().build().unwrap();
        timing("http client", started);
        let started = Instant::now();
        let health = format!("{base_url}/health");
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if client.get(&health).send().await.is_ok_and(|r| r.status().is_success()) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        }).await.expect("Core HTTP server did not become healthy");
        timing("http readiness", started);
        let started = Instant::now();

        let body = fixture_session(&runtime, "admin@test.local").await;
        let token = body["accessToken"].as_str().unwrap().to_string();

        // The seeded assistant takes the first admin slot. This human fixture
        // needs control-plane access before installing any app.
        sqlx::query(
            "INSERT INTO rootcx_system.rbac_assignments (user_id, role)
             SELECT id, 'admin' FROM rootcx_system.users WHERE email = 'admin@test.local'
             ON CONFLICT DO NOTHING",
        )
        .execute(runtime.pool())
        .await
        .expect("assign harness admin role");

        timing("http/auth", started);
        Self {
            base_url,
            client,
            runtime,
            token,
            server,
            _tmp: tmp,
            _database: database,
        }
    }

    pub fn pool(&self) -> &sqlx::PgPool {
        self.runtime.pool()
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    fn authed(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        req.bearer_auth(&self.token)
    }

    async fn send_json(&self, method: Method, path: &str, body: &Value) -> (StatusCode, Value) {
        self.request_as(method, path, &self.token, Some(body)).await
    }

    pub async fn get_json(&self, path: &str) -> (StatusCode, Value) {
        self.request_as(Method::GET, path, &self.token, None).await
    }

    pub async fn post_json(&self, path: &str, body: &Value) -> (StatusCode, Value) {
        self.send_json(Method::POST, path, body).await
    }

    pub async fn patch_json(&self, path: &str, body: &Value) -> (StatusCode, Value) {
        self.send_json(Method::PATCH, path, body).await
    }

    pub async fn put_json(&self, path: &str, body: &Value) -> (StatusCode, Value) {
        self.send_json(Method::PUT, path, body).await
    }

    pub async fn delete(&self, path: &str) -> StatusCode {
        self.authed(self.client.delete(self.url(path)))
            .send()
            .await
            .unwrap()
            .status()
    }

    pub async fn delete_json(&self, path: &str) -> (StatusCode, Value) {
        self.request_as(Method::DELETE, path, &self.token, None).await
    }

    pub async fn upload(
        &self,
        path: &str,
        name: &str,
        mime: &str,
        data: &[u8],
    ) -> (StatusCode, Value) {
        let part = multipart::Part::bytes(data.to_vec())
            .file_name(name.to_string())
            .mime_str(mime)
            .unwrap();
        let form = multipart::Form::new().part("file", part);
        let r = self
            .authed(self.client.post(self.url(path)))
            .multipart(form)
            .send()
            .await
            .unwrap();
        let s = r.status();
        (s, r.json().await.unwrap_or(Value::Null))
    }

    pub async fn get_raw(&self, path: &str) -> (StatusCode, Vec<u8>, String) {
        let r = self
            .authed(self.client.get(self.url(path)))
            .send()
            .await
            .unwrap();
        let s = r.status();
        let ct = r
            .headers()
            .get("content-type")
            .map(|v| v.to_str().unwrap_or(""))
            .unwrap_or("")
            .to_string();
        let bytes = r.bytes().await.unwrap_or_default().to_vec();
        (s, bytes, ct)
    }

    pub async fn get_unauthed(&self, path: &str) -> StatusCode {
        self.client
            .get(self.url(path))
            .send()
            .await
            .unwrap()
            .status()
    }

    pub async fn post_unauthed(&self, path: &str, body: &Value) -> (StatusCode, Value) {
        let r = self
            .client
            .post(self.url(path))
            .json(body)
            .send()
            .await
            .unwrap();
        let s = r.status();
        (s, r.json().await.unwrap_or(Value::Null))
    }

    pub async fn delete_unauthed(&self, path: &str) -> StatusCode {
        self.client
            .delete(self.url(path))
            .send()
            .await
            .unwrap()
            .status()
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
        self.install_manifest(&manifest).await;
    }

    pub async fn install_manifest(&self, manifest: &Value) {
        let (s, body) = self.post_json("/api/v1/apps", manifest).await;
        assert_eq!(s, 200, "install_manifest failed: {body}");
    }

    pub async fn create(&self, app: &str, entity: &str, body: &Value) -> Value {
        let (s, v) = self
            .post_json(&format!("/api/v1/apps/{app}/collections/{entity}"), body)
            .await;
        assert_eq!(s, 201, "create {app}/{entity} failed: {v}");
        v
    }

    pub async fn deploy(&self, app_id: &str, data: &[u8]) -> (StatusCode, Value) {
        self.upload(
            &format!("/api/v1/apps/{app_id}/deploy"),
            "backend.tar.gz",
            "application/gzip",
            data,
        )
        .await
    }

    pub async fn create_user(&self, email: &str) -> String {
        self.user_session(email).await["accessToken"].as_str().unwrap().to_string()
    }

    pub async fn user_session(&self, email: &str) -> Value {
        fixture_session(&self.runtime, email).await
    }

    pub async fn request_as(
        &self,
        method: Method,
        path: &str,
        token: &str,
        body: Option<&Value>,
    ) -> (StatusCode, Value) {
        let mut req = self
            .client
            .request(method, self.url(path))
            .bearer_auth(token);
        if let Some(b) = body {
            req = req.json(b);
        }
        let r = req.send().await.unwrap();
        let s = r.status();
        (s, r.json().await.unwrap_or(Value::Null))
    }

    pub async fn shutdown(self) {
        let started = Instant::now();
        self.server.abort();
        let _ = self.server.await;
        self.runtime.shutdown().await;
        timing("shutdown", started);
    }
}

async fn fixture_session(runtime: &ReadyRuntime, email: &str) -> Value {
    let user_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO rootcx_system.users (email) VALUES ($1)
         ON CONFLICT (email) DO UPDATE SET email = EXCLUDED.email RETURNING id",
    ).bind(email).fetch_one(runtime.pool()).await.unwrap();
    let session_id = uuid::Uuid::new_v4();
    let config = runtime.auth_config();
    sqlx::query("INSERT INTO rootcx_system.sessions (id, user_id, expires_at) VALUES ($1, $2, $3)")
        .bind(session_id).bind(user_id).bind(chrono::Utc::now() + config.refresh_ttl)
        .execute(runtime.pool()).await.unwrap();
    json!({
        "accessToken": rootcx_core::auth::jwt::encode_access(config, user_id, email).unwrap(),
        "refreshToken": rootcx_core::auth::jwt::encode_refresh(config, user_id, session_id).unwrap(),
        "expiresIn": config.access_ttl.as_secs(),
    })
}

fn timing(phase: &str, started: Instant) {
    if std::env::var_os("TEST_TIMINGS").is_some() {
        let descriptors = std::fs::read_dir("/dev/fd").ok().map(|entries| entries.count());
        eprintln!(
            "[test timing] {} {phase}: {:?}, open descriptors: {descriptors:?}",
            std::thread::current().name().unwrap_or("unknown"), started.elapsed(),
        );
    }
}

async fn fresh_database() -> (PgConnection, String) {
    let url = std::env::var("TEST_DATABASE_URL").expect("run integration tests with make test-integration");
    let options: PgConnectOptions = url.parse().expect("valid test database URL");
    assert_eq!(options.get_database(), Some("rootcx_test"), "refusing to reset a non-test database");
    let database = tokio::time::timeout(Duration::from_secs(45), async {
        let mut admin = PgConnection::connect_with(&options.database("postgres")).await.unwrap();
        // pg_cron owns one database per server. Keep it real and serialize resets,
        // including when the test binary is invoked without nextest.
        sqlx::query("SELECT pg_advisory_lock(82409125)")
            .execute(&mut admin).await.unwrap();
        sqlx::query("DROP DATABASE IF EXISTS rootcx_test WITH (FORCE)")
            .execute(&mut admin).await.unwrap();
        sqlx::query("CREATE DATABASE rootcx_test")
            .execute(&mut admin).await.unwrap();
        admin
    }).await.expect("test database reset exceeded 45 seconds");
    (database, url)
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[allow(dead_code)]
pub fn make_tar_gz(files: &[(&str, &[u8])]) -> Vec<u8> {
    let enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut tar = tar::Builder::new(enc);
    for &(name, data) in files {
        let mut header = tar::Header::new_gnu();
        header.set_path(name).unwrap();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append(&header, data).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap()
}
