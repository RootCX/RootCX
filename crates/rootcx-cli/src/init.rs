use anyhow::{Result, Context, bail};
use std::path::Path;
use std::time::Duration;

use crate::{archive, bun, config, deploy, docker, logo, oidc};

fn cloud_url() -> String {
    std::env::var("ROOTCX_CLOUD_URL").unwrap_or_else(|_| "https://rootcx.com".into())
}

pub async fn run() -> Result<()> {
    logo::print();

    let app_name: String = cliclack::input("App name")
        .placeholder("my_app")
        .validate(|v: &String| {
            if v.is_empty() { return Err("required"); }
            if !v.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
                return Err("letters, numbers, _ or -");
            }
            Ok(())
        })
        .interact()?;

    let provider: &str = cliclack::select("LLM Provider")
        .item("anthropic", "Anthropic", "")
        .item("openai", "OpenAI", "")
        .item("bedrock", "AWS Bedrock", "")
        .interact()?;
    let llm = collect_llm(provider)?;

    let target: &str = cliclack::select("Where do you want to run?")
        .item("cloud", "RootCX Cloud (free)", "")
        .item("local", "Self-host (Docker)", "")
        .interact()?;

    let (core_url, access_token, refresh_token) = match target {
        "cloud" => setup_cloud().await?,
        _ => setup_selfhost().await?,
    };

    configure_llm(&core_url, &access_token, &llm).await?;

    let cwd = std::env::current_dir()?;
    let app_dir = cwd.join(&app_name);
    if app_dir.exists() { bail!("{app_name}/ already exists"); }
    scaffold(&app_dir, &app_name).await?;
    save_config(&app_dir, &core_url, &access_token, refresh_token)?;
    cliclack::log::success(format!("Scaffolded {app_name}/"))?;

    deploy_app(&app_dir, &app_name, &core_url, &access_token).await?;

    cliclack::outro(format!("Live at {core_url}/apps/{app_name}"))?;
    eprintln!("\n  cd {app_name} && rootcx deploy   (to redeploy)");
    Ok(())
}

// -- LLM config ---------------------------------------------------------------

struct Llm {
    provider_id: String,
    name: String,
    model: String,
    secrets: Vec<(String, String)>,
}

fn collect_llm(provider: &str) -> Result<Llm> {
    match provider {
        "anthropic" => Ok(Llm {
            provider_id: "anthropic".into(), name: "Anthropic".into(),
            model: "claude-sonnet-4-6".into(),
            secrets: vec![("ANTHROPIC_API_KEY".into(), cliclack::password("ANTHROPIC_API_KEY").mask('▪').interact()?)],
        }),
        "openai" => Ok(Llm {
            provider_id: "openai".into(), name: "OpenAI".into(),
            model: "gpt-4.1".into(),
            secrets: vec![("OPENAI_API_KEY".into(), cliclack::password("OPENAI_API_KEY").mask('▪').interact()?)],
        }),
        "bedrock" => collect_bedrock(),
        _ => unreachable!(),
    }
}

fn collect_bedrock() -> Result<Llm> {
    let mode: &str = cliclack::select("AWS Auth Mode")
        .item("apikey", "API Key", "")
        .item("iam", "IAM Credentials", "")
        .interact()?;
    let secrets = if mode == "apikey" {
        vec![
            ("AWS_BEARER_TOKEN_BEDROCK".into(), cliclack::password("AWS_BEARER_TOKEN_BEDROCK").mask('▪').interact()?),
            ("AWS_DEFAULT_REGION".into(), cliclack::input("AWS_DEFAULT_REGION").default_input("us-east-1").interact()?),
        ]
    } else {
        vec![
            ("AWS_ACCESS_KEY_ID".into(), cliclack::password("AWS_ACCESS_KEY_ID").mask('▪').interact()?),
            ("AWS_SECRET_ACCESS_KEY".into(), cliclack::password("AWS_SECRET_ACCESS_KEY").mask('▪').interact()?),
            ("AWS_DEFAULT_REGION".into(), cliclack::input("AWS_DEFAULT_REGION").default_input("us-east-1").interact()?),
        ]
    };
    Ok(Llm {
        provider_id: "bedrock".into(), name: "AWS Bedrock".into(),
        model: "us.anthropic.claude-sonnet-4-6".into(), secrets,
    })
}

// -- Cloud setup (same flow as website UI) ------------------------------------

async fn setup_cloud() -> Result<(String, String, Option<String>)> {
    let email: String = cliclack::input("Email").interact()?;
    let password: String = cliclack::password("Password").mask('▪').interact()?;
    let name = email.split('@').next().unwrap_or("user").to_string();
    let website = cloud_url();

    // Cookie jar: same session across all requests, exactly like a browser
    let http = reqwest::Client::builder().cookie_store(true).build()?;

    // 1. Register (ignore 409 = already exists)
    let reg_resp = http.post(format!("{website}/api/auth/register"))
        .json(&serde_json::json!({ "email": email, "password": password, "name": name }))
        .send().await.context("register")?;
    let is_new = reg_resp.status().is_success();

    // 2. NextAuth sign-in: get CSRF token, then POST credentials (sets session cookie)
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

    // 3. Create project (POST /api/projects, same endpoint the website uses)
    let ws_name = name.to_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>();
    let proj_resp = http.post(format!("{website}/api/projects"))
        .json(&serde_json::json!({ "name": ws_name, "plan": "free" }))
        .send().await.context("create project")?;
    if !proj_resp.status().is_success() {
        bail!("create project: {}", proj_resp.text().await.unwrap_or_default());
    }
    let project: serde_json::Value = proj_resp.json().await?;
    let project_ref = project["ref"].as_str().context("no ref in response")?.to_string();

    // 4. Poll until active (GET /api/projects, same as website dashboard)
    let sp = cliclack::spinner();
    sp.start("Setting up workspace...");
    let api_url = poll_project(&http, &website, &project_ref).await?;
    sp.stop("Workspace ready");

    // 5. OIDC auth against the Core (existing flow, opens browser)
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

// -- Self-host setup ----------------------------------------------------------

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

    let pw = auto_password();
    let (at, rt) = register_on_core(docker::LOCAL_URL, "admin@localhost", &pw).await?;
    Ok((docker::LOCAL_URL.into(), at, Some(rt)))
}

async fn register_on_core(base: &str, email: &str, password: &str) -> Result<(String, String)> {
    let http = reqwest::Client::new();
    let _ = http.post(format!("{base}/api/v1/auth/register"))
        .json(&serde_json::json!({ "email": email, "password": password }))
        .send().await;
    let resp = http.post(format!("{base}/api/v1/auth/login"))
        .json(&serde_json::json!({ "email": email, "password": password }))
        .send().await.context("Core login")?;
    if !resp.status().is_success() {
        bail!("Core login failed: {}", resp.text().await.unwrap_or_default());
    }
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct R { access_token: String, refresh_token: String }
    let r: R = resp.json().await?;
    Ok((r.access_token, r.refresh_token))
}

// -- LLM on Core --------------------------------------------------------------

async fn configure_llm(url: &str, token: &str, llm: &Llm) -> Result<()> {
    let http = reqwest::Client::new();
    for (k, v) in &llm.secrets {
        let r = http.post(format!("{url}/api/v1/platform/secrets"))
            .bearer_auth(token)
            .json(&serde_json::json!({ "key": k, "value": v }))
            .send().await.with_context(|| format!("set secret {k}"))?;
        if !r.status().is_success() {
            bail!("set secret {k}: {}", r.text().await.unwrap_or_default());
        }
    }
    let r = http.post(format!("{url}/api/v1/llm-models"))
        .bearer_auth(token)
        .json(&serde_json::json!({
            "id": llm.provider_id, "name": llm.name,
            "provider": llm.provider_id, "model": llm.model,
            "is_default": true,
        }))
        .send().await.context("create LLM model")?;
    if !r.status().is_success() {
        bail!("create LLM model: {}", r.text().await.unwrap_or_default());
    }
    Ok(())
}

// -- Scaffold + Deploy --------------------------------------------------------

async fn scaffold(dir: &Path, name: &str) -> Result<()> {
    let reg = rootcx_scaffold::Registry::new();
    let preset = reg.get("blank").map_err(|e| anyhow::anyhow!(e))?;
    let answers = preset.questions().into_iter()
        .filter_map(|q| q.default.map(|d| (q.key, d))).collect();
    rootcx_scaffold::create(dir, name, "blank", answers, vec![])
        .await.map_err(|e| anyhow::anyhow!(e))
}

fn save_config(app_dir: &Path, url: &str, token: &str, refresh: Option<String>) -> Result<()> {
    let cfg = config::Config { url: url.into(), token: Some(token.into()), refresh_token: refresh };
    let dir = app_dir.join(".rootcx");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("config.json"), serde_json::to_string_pretty(&cfg)?)?;
    Ok(())
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

fn auto_password() -> String {
    format!("rootcx-{:016x}", std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos())
}
