use std::net::TcpListener;
use std::sync::Arc;

use reqwest::{Client, Method, StatusCode, multipart};
use rootcx_core::{ReadyRuntime, Runtime, server};
use serde_json::{Value, json};
use tempfile::TempDir;
use testcontainers::{ContainerAsync, GenericImage, ImageExt, runners::AsyncRunner};
use testcontainers::core::{IntoContainerPort, WaitFor};

const PG_IMAGE: &str = "ghcr.io/rootcx/postgresql";
const PG_TAG: &str = "16-pgmq-cron";

pub struct TestRuntime {
    base_url: String,
    pub client: Client,
    runtime: Arc<ReadyRuntime>,
    pub token: String,
    _tmp: TempDir,
    _container: ContainerAsync<GenericImage>,
}

impl TestRuntime {
    pub async fn boot() -> Self {
        let resources = rootcx_platform::dirs::resources_dir(env!("CARGO_MANIFEST_DIR"))
            .expect("resources dir not found");
        let bun_bin = rootcx_platform::bin::binary_path(&resources, "bun");
        let tmp = TempDir::new().unwrap();
        let data_dir = tmp.path().to_path_buf();
        let api_port = free_port();

        let container = GenericImage::new(PG_IMAGE, PG_TAG)
            .with_exposed_port(5432_u16.tcp())
            .with_wait_for(WaitFor::message_on_stderr("database system is ready to accept connections"))
            .with_entrypoint("/pg-entrypoint.sh")
            .with_user("root")
            .with_env_var("POSTGRES_USER", "rootcx")
            .with_env_var("POSTGRES_PASSWORD", "rootcx")
            .with_env_var("POSTGRES_DB", "rootcx")
            .with_env_var("PGDATA", "/tmp/pgdata")
            .start()
            .await
            .expect("failed to start postgres container");

        let pg_port = container.get_host_port_ipv4(5432).await.unwrap();
        let db_url = format!("postgresql://rootcx:rootcx@127.0.0.1:{pg_port}/rootcx");

        let resources_dir = data_dir.join("resources");
        std::fs::create_dir_all(&resources_dir).unwrap();
        let runtime = Arc::new(
            Runtime::new(db_url, data_dir, resources_dir, bun_bin)
                .boot(api_port).await.expect("boot failed")
        );
        let rt = Arc::clone(&runtime);
        tokio::spawn(async move { server::serve(rt, api_port).await.ok(); });

        let base_url = format!("http://127.0.0.1:{api_port}");
        let client = Client::new();
        let health = format!("{base_url}/health");
        for _ in 0..100 {
            if client.get(&health).send().await.is_ok() { break; }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }

        let creds = json!({"email":"admin@test.local","password":"Str0ngPass1"});
        client.post(format!("{base_url}/api/v1/auth/register")).json(&creds).send().await.ok();
        let res = client.post(format!("{base_url}/api/v1/auth/login")).json(&creds).send().await.unwrap();
        let body: Value = res.json().await.unwrap();
        let token = body["accessToken"].as_str().unwrap().to_string();

        Self { base_url, client, runtime, token, _tmp: tmp, _container: container }
    }

    pub fn pool(&self) -> &sqlx::PgPool { self.runtime.pool() }

    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    fn authed(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        req.bearer_auth(&self.token)
    }

    async fn send_json(&self, method: Method, path: &str, body: &Value) -> (StatusCode, Value) {
        let r = self.authed(self.client.request(method, self.url(path)))
            .json(body).send().await.unwrap();
        let s = r.status();
        (s, r.json().await.unwrap_or(Value::Null))
    }

    pub async fn get_json(&self, path: &str) -> (StatusCode, Value) {
        let r = self.authed(self.client.get(self.url(path))).send().await.unwrap();
        let s = r.status();
        (s, r.json().await.unwrap_or(Value::Null))
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
        self.authed(self.client.delete(self.url(path))).send().await.unwrap().status()
    }

    pub async fn delete_json(&self, path: &str) -> (StatusCode, Value) {
        let r = self.authed(self.client.delete(self.url(path))).send().await.unwrap();
        let s = r.status();
        (s, r.json().await.unwrap_or(Value::Null))
    }

    pub async fn upload(&self, path: &str, name: &str, mime: &str, data: &[u8]) -> (StatusCode, Value) {
        let part = multipart::Part::bytes(data.to_vec()).file_name(name.to_string()).mime_str(mime).unwrap();
        let form = multipart::Form::new().part("file", part);
        let r = self.authed(self.client.post(self.url(path))).multipart(form).send().await.unwrap();
        let s = r.status();
        (s, r.json().await.unwrap_or(Value::Null))
    }

    pub async fn get_unauthed(&self, path: &str) -> StatusCode {
        self.client.get(self.url(path)).send().await.unwrap().status()
    }

    pub async fn post_unauthed(&self, path: &str, body: &Value) -> (StatusCode, Value) {
        let r = self.client.post(self.url(path)).json(body).send().await.unwrap();
        let s = r.status();
        (s, r.json().await.unwrap_or(Value::Null))
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
        self.install_manifest(&manifest).await;
    }

    pub async fn install_manifest(&self, manifest: &Value) {
        let (s, body) = self.post_json("/api/v1/apps", manifest).await;
        assert_eq!(s, 200, "install_manifest failed: {body}");
    }

    pub async fn create(&self, app: &str, entity: &str, body: &Value) -> Value {
        let (s, v) = self.post_json(&format!("/api/v1/apps/{app}/collections/{entity}"), body).await;
        assert_eq!(s, 201, "create {app}/{entity} failed: {v}");
        v
    }

    pub async fn deploy(&self, app_id: &str, data: &[u8]) -> (StatusCode, Value) {
        self.upload(&format!("/api/v1/apps/{app_id}/deploy"), "backend.tar.gz", "application/gzip", data).await
    }

    pub async fn shutdown(self) {
        self.runtime.shutdown().await;
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}
