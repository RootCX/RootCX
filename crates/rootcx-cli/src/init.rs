use anyhow::{Result, Context, bail};
use std::path::Path;

use crate::{archive, bun, config, deploy, docker, logo};

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
    let url = format!("{}/app/projects", cloud_url().trim_end_matches('/'));
    cliclack::log::step("Create or select your workspace in the browser")?;
    if webbrowser::open(&url).is_err() {
        cliclack::log::info(format!("Open {url}"))?;
    }
    let workspace: String = cliclack::input("Workspace URL")
        .placeholder("https://your-workspace.rootcx.com")
        .validate(|value: &String| validate_workspace_url(value))
        .interact()?;
    connect_workspace(&workspace).await
}

async fn connect_workspace(base: &str) -> Result<(String, String, Option<String>)> {
    crate::auth::connect(base, None).await?;
    let cfg = config::load()?;
    let token = cfg.token.context("workspace authentication did not complete")?;
    Ok((cfg.url, token, cfg.refresh_token))
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

    connect_workspace(docker::LOCAL_URL).await
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

fn validate_workspace_url(value: &str) -> Result<(), &'static str> {
    match reqwest::Url::parse(value) {
        Ok(url) if matches!(url.scheme(), "http" | "https") && url.host_str().is_some()
            && url.username().is_empty() && url.password().is_none() => Ok(()),
        _ => Err("Enter a workspace HTTP(S) URL"),
    }
}

async fn scaffold(dir: &Path, name: &str) -> Result<()> {
    let reg = rootcx_scaffold::Registry::new();
    let preset = reg.get("blank").map_err(|e| anyhow::anyhow!(e))?;
    let answers = preset.questions().into_iter()
        .filter_map(|q| q.default.map(|d| (q.key, d))).collect();
    rootcx_scaffold::create(dir, name, "blank", answers, vec![])
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

    #[test]
    fn workspace_url_requires_http_without_embedded_credentials() {
        for (input, valid) in [
            ("https://kova.rootcx.com", true),
            ("http://localhost:9100", true),
            ("", false),
            ("/app/projects", false),
            ("not a URL", false),
            ("javascript:alert(1)", false),
            ("file:///tmp/core", false),
            ("https://user:secret@kova.rootcx.com", false),
        ] {
            assert_eq!(validate_workspace_url(input).is_ok(), valid, "{input:?}");
        }
    }

    struct TestCore {
        base_url: String,
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
        TestCore { base_url, _container: container, _tmp: tmp, _rt: rt }
    }

    async fn authed_client(c: &TestCore, email: &str) -> rootcx_client::RuntimeClient {
        let id = sqlx::query_scalar("INSERT INTO rootcx_system.users (email) VALUES ($1) RETURNING id")
            .bind(email).fetch_one(c._rt.pool()).await.unwrap();
        let access = rootcx_core::auth::jwt::encode_access(c._rt.auth_config(), id, email).unwrap();
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
