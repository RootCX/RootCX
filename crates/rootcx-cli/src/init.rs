use anyhow::{Result, Context, bail};
use std::path::Path;
use std::time::Duration;

use crate::{archive, bun, config, deploy, docker, logo, oidc};

fn cloud_url() -> String {
    std::env::var("ROOTCX_CLOUD_URL").unwrap_or_else(|_| "https://rootcx.com".into())
}

pub async fn run() -> Result<()> {
    cliclack::set_theme(crate::theme::RootcxTheme);
    logo::print();

    let app_name: String = cliclack::input("App name")
        .placeholder("my_app")
        .validate(|v: &String| validate_app_name(v))
        .interact()?;

    let (core_url, access_token, refresh_token) = if let Some(session) = try_existing_session().await {
        cliclack::log::success(format!("Connected to {}", session.0))?;
        session
    } else {
        let target: &str = cliclack::select("Where do you want to run?")
            .item("cloud", "RootCX Cloud (free)", "")
            .item("local", "Self-host (Docker)", "")
            .interact()?;
        match target {
            "cloud" => setup_cloud().await?,
            _ => setup_selfhost().await?,
        }
    };

    let cwd = std::env::current_dir()?;
    let app_dir = cwd.join(&app_name);
    if app_dir.exists() { bail!("{app_name}/ already exists"); }
    scaffold(&app_dir, &app_name).await?;
    config::save(&config::Config { url: core_url.clone(), token: Some(access_token.clone()), refresh_token })?;
    cliclack::log::success(format!("Scaffolded {app_name}/"))?;

    deploy_app(&app_dir, &app_name, &core_url, &access_token).await?;

    cliclack::outro(format!("Your app is live!  {core_url}/apps/{app_name}"))?;
    eprintln!("\n  Next steps\n");
    eprintln!("    cd {app_name}/");
    eprintln!("    Open your AI code editor (e.g. claude) and start prompting!");
    eprintln!("    rootcx deploy to push changes\n");
    Ok(())
}

async fn setup_cloud() -> Result<(String, String, Option<String>)> {
    let email: String = cliclack::input("Email").interact()?;
    let password: String = cliclack::password("Password").mask('▪').interact()?;
    let name = email.split('@').next().unwrap_or("user").to_string();
    let website = cloud_url();

    let http = reqwest::Client::builder().cookie_store(true).build()?;

    let reg_resp = http.post(format!("{website}/api/auth/register"))
        .json(&serde_json::json!({ "email": email, "password": password, "name": name }))
        .send().await.context("register")?;
    let is_new = reg_resp.status().is_success();

    let csrf_resp = http.get(format!("{website}/api/auth/csrf"))
        .send().await.context("csrf")?;
    let csrf: serde_json::Value = csrf_resp.json().await?;
    let csrf_token = csrf["csrfToken"].as_str().context("no csrfToken")?;

    let login_resp = http.post(format!("{website}/api/auth/callback/credentials"))
        .form(&[
            ("email", email.as_str()),
            ("password", password.as_str()),
            ("csrfToken", csrf_token),
            ("redirect", "false"),
            ("json", "true"),
        ])
        .send().await.context("sign in")?;
    if !login_resp.status().is_success() {
        bail!("login failed: {}", login_resp.text().await.unwrap_or_default());
    }

    if is_new { cliclack::log::success("Account created")?; }
    cliclack::log::success(format!("Logged in as {email}"))?;

    let ws_name = name.to_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>();
    let proj_resp = http.post(format!("{website}/api/projects"))
        .json(&serde_json::json!({ "name": ws_name, "plan": "free" }))
        .send().await.context("create project")?;
    if !proj_resp.status().is_success() {
        bail!("create project: {}", proj_resp.text().await.unwrap_or_default());
    }
    let project: serde_json::Value = proj_resp.json().await?;
    let project_ref = project["ref"].as_str().context("no ref in response")?.to_string();

    let sp = cliclack::spinner();
    sp.start("Provisioning workspace (database, networking, DNS)... this may take a few minutes");
    let api_url = poll_project(&http, &website, &project_ref).await?;
    sp.stop("Workspace ready");

    let mode_resp = reqwest::get(format!("{api_url}/api/v1/auth/mode"))
        .await.context("Core unreachable")?;
    let mode: serde_json::Value = mode_resp.json().await?;
    let provider_id = mode["providers"].as_array()
        .and_then(|p| p.first())
        .and_then(|p| p["id"].as_str())
        .context("Core has no OIDC provider")?;
    let tokens = oidc::login(&api_url, provider_id).await?;

    Ok((api_url, tokens.access_token, Some(tokens.refresh_token)))
}

async fn poll_project(http: &reqwest::Client, website: &str, project_ref: &str) -> Result<String> {
    for _ in 0..120 {
        if let Ok(resp) = http.get(format!("{website}/api/projects")).send().await {
            if resp.status().is_success() {
                if let Ok(list) = resp.json::<Vec<serde_json::Value>>().await {
                    if let Some(p) = list.iter().find(|p| p["ref"].as_str() == Some(project_ref)) {
                        match p["status"].as_str() {
                            Some("active") => {
                                return p["apiUrl"].as_str()
                                    .context("project active but no apiUrl").map(String::from);
                            }
                            Some("error") => bail!("workspace provisioning failed"),
                            _ => {}
                        }
                    }
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
    bail!("provisioning timed out (6 min)")
}

async fn setup_selfhost() -> Result<(String, String, Option<String>)> {
    let sp = cliclack::spinner();
    sp.start("Checking Docker...");
    if !docker::check().await {
        sp.stop("Docker not found");
        bail!("Docker is required. Install from docker.com and try again.");
    }
    sp.stop("Docker OK");

    let sp = cliclack::spinner();
    sp.start("Starting Core...");
    docker::start_core().await?;
    sp.stop(format!("Core running at {}", docker::LOCAL_URL));

    let base = docker::LOCAL_URL;
    let http = reqwest::Client::new();

    let email: String = cliclack::input("Email").interact()?;
    let password: String = cliclack::password("Password").mask('▪').interact()?;

    let (is_new, access_token, refresh_token) = selfhost_auth(&http, base, &email, &password).await?;
    if is_new { cliclack::log::success("Account created")?; }
    cliclack::log::success(format!("Logged in as {email}"))?;
    Ok((base.into(), access_token, Some(refresh_token)))
}

async fn selfhost_auth(http: &reqwest::Client, base: &str, email: &str, password: &str) -> Result<(bool, String, String)> {
    let is_new = http.post(format!("{base}/api/v1/auth/register"))
        .json(&serde_json::json!({ "email": email, "password": password }))
        .send().await.context("register")?
        .status().is_success();

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct LoginResp { access_token: String, refresh_token: String }

    let resp = http.post(format!("{base}/api/v1/auth/login"))
        .json(&serde_json::json!({ "email": email, "password": password }))
        .send().await.context("login")?;
    if !resp.status().is_success() {
        bail!("Invalid email or password");
    }
    let r: LoginResp = resp.json().await?;
    Ok((is_new, r.access_token, r.refresh_token))
}

async fn try_existing_session() -> Option<(String, String, Option<String>)> {
    let mut cfg = config::load().ok()?;
    crate::auth::ensure_valid_token(&mut cfg).await.ok()?;
    let token = cfg.token.clone()?;
    Some((cfg.url, token, cfg.refresh_token))
}

fn validate_app_name(name: &str) -> Result<(), &'static str> {
    if name.is_empty() { return Err("required"); }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return Err("letters, numbers, _ or -");
    }
    Ok(())
}

async fn scaffold(dir: &Path, name: &str) -> Result<()> {
    let reg = rootcx_scaffold::Registry::new();
    let preset = reg.get("blank").map_err(|e| anyhow::anyhow!(e))?;
    let answers = preset.questions().into_iter()
        .filter_map(|q| q.default.map(|d| (q.key, d))).collect();

    let skills_source = config::skills_dir()?.join("rootcx");
    let extra_layers: Vec<Box<dyn rootcx_scaffold::types::Layer>> = vec![
        Box::new(rootcx_scaffold::layers::SkillLayer::new(dir.to_path_buf(), skills_source)),
    ];

    rootcx_scaffold::create(dir, name, "blank", answers, extra_layers)
        .await.map_err(|e| anyhow::anyhow!(e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::Arc;
    use testcontainers::{GenericImage, ImageExt, runners::AsyncRunner};
    use testcontainers::core::{IntoContainerPort, WaitFor};
    use testcontainers::ContainerAsync;

    #[test]
    fn app_name_validation() {
        let cases = [
            ("my_app", true),
            ("my-app", true),
            ("app123", true),
            ("A-z_0", true),
            ("", false),
            ("has space", false),
            ("has/slash", false),
            ("has.dot", false),
            ("emoji🎉", false),
        ];
        for (input, expect_ok) in cases {
            let result = validate_app_name(input);
            assert_eq!(result.is_ok(), expect_ok, "validate_app_name({input:?}) = {result:?}");
        }
    }

    struct TestCore {
        base_url: String,
        http: reqwest::Client,
        _container: ContainerAsync<GenericImage>,
        _tmp: tempfile::TempDir,
        _rt: Arc<rootcx_core::ReadyRuntime>,
    }

    async fn boot_core() -> TestCore {
        let tmp = tempfile::TempDir::new().unwrap();
        let data_dir = tmp.path().to_path_buf();
        let core_manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../core");
        let resources = rootcx_platform::dirs::resources_dir(core_manifest.to_str().unwrap())
            .expect("core/resources not found -- run `make deps` first");
        let bun_bin = rootcx_platform::bin::binary_path(&resources, "bun");
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();

        let container = GenericImage::new("ghcr.io/rootcx/postgresql", "16-pgmq-cron")
            .with_exposed_port(5432_u16.tcp())
            .with_wait_for(WaitFor::message_on_stderr("database system is ready to accept connections"))
            .with_entrypoint("/pg-entrypoint.sh")
            .with_user("root")
            .with_env_var("POSTGRES_USER", "rootcx")
            .with_env_var("POSTGRES_PASSWORD", "rootcx")
            .with_env_var("POSTGRES_DB", "rootcx")
            .with_env_var("PGDATA", "/tmp/pgdata")
            .start().await.expect("failed to start postgres");

        let pg_port = container.get_host_port_ipv4(5432).await.unwrap();
        let db_url = format!("postgresql://rootcx:rootcx@127.0.0.1:{pg_port}/rootcx");

        let rt = Arc::new(
            rootcx_core::Runtime::new(db_url, data_dir, resources, bun_bin)
                .boot(port).await.expect("boot failed")
        );
        let rt2 = Arc::clone(&rt);
        tokio::spawn(async move { rootcx_core::server::serve(rt2, port).await.ok(); });

        let base_url = format!("http://127.0.0.1:{port}");
        let http = reqwest::Client::new();
        for _ in 0..100 {
            if http.get(format!("{base_url}/health")).send().await.is_ok() { break; }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        TestCore { base_url, http, _container: container, _tmp: tmp, _rt: rt }
    }

    #[tokio::test]
    async fn selfhost_auth_new_user_registers_then_logs_in() {
        let c = boot_core().await;
        let (is_new, at, rt) = selfhost_auth(&c.http, &c.base_url, "new@test.local", "Str0ngPass1").await.unwrap();
        assert!(is_new);
        assert!(!at.is_empty());
        assert!(!rt.is_empty());
    }

    #[tokio::test]
    async fn selfhost_auth_existing_user_skips_register() {
        let c = boot_core().await;
        selfhost_auth(&c.http, &c.base_url, "twice@test.local", "Str0ngPass1").await.unwrap();
        let (is_new, at, _) = selfhost_auth(&c.http, &c.base_url, "twice@test.local", "Str0ngPass1").await.unwrap();
        assert!(!is_new);
        assert!(!at.is_empty());
    }

    #[tokio::test]
    async fn selfhost_auth_bad_password_fails() {
        let c = boot_core().await;
        selfhost_auth(&c.http, &c.base_url, "bad@test.local", "Str0ngPass1").await.unwrap();
        let err = selfhost_auth(&c.http, &c.base_url, "bad@test.local", "WrongPass99").await.unwrap_err();
        assert!(err.to_string().contains("Invalid email or password"), "got: {err}");
    }

    #[tokio::test]
    async fn selfhost_auth_short_password_fails() {
        let c = boot_core().await;
        let err = selfhost_auth(&c.http, &c.base_url, "short@test.local", "abc").await.unwrap_err();
        assert!(err.to_string().contains("Invalid email or password"), "got: {err}");
    }

    async fn authed_client(c: &TestCore, email: &str) -> rootcx_client::RuntimeClient {
        let (_, access, _) = selfhost_auth(&c.http, &c.base_url, email, "Str0ngPass1").await.unwrap();
        let client = rootcx_client::RuntimeClient::new(&c.base_url);
        client.set_token(Some(access));
        client
    }

    #[tokio::test]
    async fn runtime_client_me_returns_authenticated_user() {
        let c = boot_core().await;
        let email = "me@test.local";
        let client = authed_client(&c, email).await;

        let user = client.me().await.expect("me() should succeed for authed client");

        assert_eq!(user["email"].as_str(), Some(email));
        assert!(user["id"].as_str().is_some_and(|s| !s.is_empty()), "id missing: {user}");
    }

    #[tokio::test]
    async fn runtime_client_me_rejects_invalid_token() {
        let c = boot_core().await;
        let client = rootcx_client::RuntimeClient::new(&c.base_url);
        client.set_token(Some("not-a-real-token".into()));

        let err = client.me().await.expect_err("me() must reject an invalid token");

        match err {
            rootcx_client::ClientError::Api { status, .. } => assert_eq!(status, 401),
            other => panic!("expected 401 Api error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn runtime_client_list_apps_and_agents_parse_successfully() {
        let c = boot_core().await;
        let client = authed_client(&c, "lists@test.local").await;

        client.list_apps().await.expect("list_apps response must parse");
        client.list_all_agents().await.expect("list_all_agents response must parse");
    }
}

async fn deploy_app(app_dir: &Path, app_id: &str, url: &str, token: &str) -> Result<()> {
    let client = rootcx_client::RuntimeClient::new(url);
    client.set_token(Some(token.into()));

    if app_dir.join("package.json").exists() {
        let bun_bin = bun::ensure().await?;
        if !app_dir.join("node_modules").exists() {
            cliclack::log::step("Installing dependencies...")?;
            bun::exec(&bun_bin, app_dir, &["install"], &[]).await?;
        }
        if app_dir.join("src").exists() {
            cliclack::log::step("Building...")?;
            let base_flag = format!("--base=/apps/{app_id}/");
            bun::exec(&bun_bin, app_dir, &["run", "build", "--", &base_flag], &[("VITE_ROOTCX_URL", url)]).await?;
        }
    }

    let manifest: rootcx_types::AppManifest = serde_json::from_str(
        &std::fs::read_to_string(app_dir.join("manifest.json"))?
    ).context("invalid manifest.json")?;

    let plan = deploy::plan_deploy(app_dir);
    let sp = cliclack::spinner();
    sp.start("Deploying...");

    client.install_app(&manifest).await.context("install manifest")?;
    if plan.backend {
        let tar = archive::pack_dir(app_dir, Path::new("backend"))?;
        client.deploy_app(app_id, tar).await.context("deploy backend")?;
    }
    if plan.frontend {
        let tar = archive::pack_dir(app_dir, Path::new("dist"))?;
        client.deploy_frontend(app_id, tar).await.context("deploy frontend")?;
    }
    if plan.backend {
        client.start_worker(app_id).await.ok();
    }
    sp.stop("Deployed");
    Ok(())
}

