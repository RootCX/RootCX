use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use rootcx_client::RuntimeClient;
use rootcx_types::AppManifest;
use std::path::Path;

mod archive;
mod auth;
mod bun;
mod cmd_agents;
mod cmd_apps;
mod cmd_auth;
mod cmd_completions;
mod cmd_status;
mod config;
mod deploy;
mod docker;
mod init;
mod logo;
mod oidc;
mod scaffold;
mod sse;
mod theme;
mod upgrade;
#[cfg(test)]
mod testutil;

#[derive(Parser)]
#[command(
    name = "rootcx",
    version,
    about = "Code, deploy and manage RootCX apps.",
    override_help = ROOT_HELP,
    disable_help_subcommand = true,
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

const ROOT_HELP: &str = "\
Code, deploy and manage RootCX apps.

USAGE
  rootcx <command> <subcommand> [flags]

CORE COMMANDS
  init         Create, scaffold, and deploy a new app
  new          Scaffold an app in a new directory
  deploy       Deploy the current project
  status       Show connection and project status

APP MANAGEMENT
  apps         Manage installed apps
  agents       Manage agents

AUTHENTICATION
  auth         Manage authentication (login, logout, whoami)

ADDITIONAL
  upgrade      Update rootcx to the latest version
  completions  Generate shell completions

FLAGS
  -h, --help     Print help for a command
  -V, --version  Print version

ALIASES
  i  -> init
  n  -> new
  d  -> deploy
  s  -> status

EXAMPLES
  $ rootcx init
  $ rootcx new my-app && cd my-app
  $ rootcx deploy
  $ rootcx auth login https://core.example.com
  $ rootcx apps list
  $ rootcx agents invoke my_app \"hello\"

LEARN MORE
  Use `rootcx <command> --help` for details on a command.
  Docs: https://rootcx.com/docs
";

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Create, scaffold, and deploy a new app
    #[command(alias = "i")]
    Init,
    /// Scaffold an app in a new directory
    #[command(alias = "n")]
    New {
        /// Name of the app (used as directory and app_id)
        name: String,
    },
    /// Deploy the current project
    #[command(alias = "d")]
    Deploy,
    /// Show connection and project status
    #[command(alias = "s")]
    Status {
        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Manage installed apps
    #[command(subcommand, override_help = APPS_HELP, disable_help_subcommand = true)]
    Apps(AppsCmd),
    /// Manage agents
    #[command(subcommand, override_help = AGENTS_HELP, disable_help_subcommand = true)]
    Agents(AgentsCmd),
    /// Manage authentication
    #[command(subcommand, override_help = AUTH_HELP, disable_help_subcommand = true)]
    Auth(AuthCmd),
    /// Update rootcx to the latest version
    Upgrade {
        /// Install a specific version (e.g. 0.9.1)
        #[arg(long)]
        version: Option<String>,
    },
    /// Generate shell completions
    Completions {
        /// Target shell
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
    /// Print the path to bundled skills
    #[command(hide = true)]
    SkillsPath,
}

const AUTH_HELP: &str = "\
Manage authentication with a RootCX Core.

USAGE
  rootcx auth <command> [flags]

COMMANDS
  login   Sign in to a Core (prompts for credentials if needed)
  logout  Clear stored credentials locally
  whoami  Show the currently signed-in user

EXAMPLES
  $ rootcx auth login https://core.example.com
  $ rootcx auth login                # reuse saved URL
  $ rootcx auth whoami
  $ rootcx auth logout
";

const APPS_HELP: &str = "\
Manage installed apps on the connected Core.

USAGE
  rootcx apps <command> [flags]

COMMANDS
  list  List installed apps (alias: ls)
  rm    Uninstall an app (requires confirmation)

EXAMPLES
  $ rootcx apps list
  $ rootcx apps list --json
  $ rootcx apps rm my_app
  $ rootcx apps rm my_app -y        # skip confirmation
";

const AGENTS_HELP: &str = "\
Invoke and inspect agents on the connected Core.

USAGE
  rootcx agents <command> [flags]

COMMANDS
  invoke    Invoke an agent and stream the response
  list      List all deployed agents (alias: ls)
  sessions  List sessions for an app's agent

EXAMPLES
  $ rootcx agents list
  $ rootcx agents invoke my_app \"summarize yesterday's leads\"
  $ rootcx agents invoke my_app \"continue\" --session 7f2c...
  $ rootcx agents sessions my_app
";

#[derive(Subcommand, Debug)]
enum AuthCmd {
    /// Sign in to a Core
    Login {
        /// Core URL (defaults to saved config)
        url: Option<String>,
        /// Provide an access token non-interactively
        #[arg(long)]
        token: Option<String>,
    },
    /// Clear stored credentials
    Logout,
    /// Show the currently signed-in user
    Whoami {
        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
enum AppsCmd {
    /// List installed apps
    #[command(alias = "ls")]
    List {
        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Uninstall an app
    Rm {
        /// App ID to uninstall
        app_id: String,
        /// Skip confirmation prompt
        #[arg(short = 'y', long)]
        yes: bool,
    },
}

#[derive(Subcommand, Debug)]
enum AgentsCmd {
    /// Invoke an agent and stream the response
    Invoke {
        /// App ID whose agent should be invoked
        app_id: String,
        /// Message to send to the agent
        message: String,
        /// Resume an existing session by ID (otherwise a new session is created)
        #[arg(long)]
        session: Option<String>,
    },
    /// List all agents
    #[command(alias = "ls")]
    List {
        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// List sessions for an app's agent
    Sessions {
        /// App ID to list sessions for
        app_id: String,
        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    if !matches!(cli.cmd, Cmd::Upgrade { .. } | Cmd::SkillsPath | Cmd::Completions { .. }) {
        tokio::spawn(upgrade::check_passive());
    }
    match cli.cmd {
        Cmd::Init => init::run().await,
        Cmd::New { name } => scaffold::run(&name, &std::env::current_dir()?).await,
        Cmd::Deploy => run_deploy().await,
        Cmd::Status { json } => cmd_status::run(json).await,
        Cmd::Auth(sub) => match sub {
            AuthCmd::Login { url, token } => cmd_auth::login(url.as_deref(), token).await,
            AuthCmd::Logout => cmd_auth::logout(),
            AuthCmd::Whoami { json } => cmd_auth::whoami(json).await,
        },
        Cmd::Apps(sub) => match sub {
            AppsCmd::List { json } => cmd_apps::list(json).await,
            AppsCmd::Rm { app_id, yes } => cmd_apps::rm(&app_id, yes).await,
        },
        Cmd::Agents(sub) => match sub {
            AgentsCmd::Invoke { app_id, message, session } => {
                sse::invoke(&client_from_config().await?, &app_id, &message, session.as_deref()).await
            }
            AgentsCmd::List { json } => cmd_agents::list(json).await,
            AgentsCmd::Sessions { app_id, json } => cmd_agents::sessions(&app_id, json).await,
        },
        Cmd::Completions { shell } => cmd_completions::run(shell),
        Cmd::Upgrade { version } => upgrade::run(version).await,
        Cmd::SkillsPath => {
            println!("{}", config::skills_dir()?.display());
            Ok(())
        }
    }
}

pub(crate) async fn client_from_config() -> Result<RuntimeClient> {
    let mut cfg = config::load().context("not signed in. Run `rootcx auth login <url>` or `rootcx init` first")?;
    auth::ensure_valid_token(&mut cfg).await?;
    let client = RuntimeClient::new(&cfg.url);
    if let Some(t) = cfg.token {
        client.set_token(Some(t));
    }
    Ok(client)
}

async fn run_deploy() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let manifest_path = cwd.join("manifest.json");
    let raw = std::fs::read_to_string(&manifest_path)
        .context(format!("no manifest.json in {}. Run `rootcx new <name>` first", cwd.display()))?;
    let manifest: AppManifest = serde_json::from_str(&raw)
        .context("invalid manifest.json")?;
    let app_id = manifest.app_id.clone();

    let client = client_from_config().await?;
    let core_url = client.base_url();

    if cwd.join("package.json").exists() {
        let bun = bun::ensure().await?;

        if !cwd.join("node_modules").exists() {
            println!("→ installing dependencies");
            bun::exec(&bun, &cwd, &["install"], &[]).await?;
        }

        if cwd.join("src").exists() {
            println!("→ building frontend");
            let base_flag = format!("--base=/apps/{app_id}/");
            let args = &["run", "build", "--", &base_flag];
            bun::exec(&bun, &cwd, args, &[("VITE_ROOTCX_URL", core_url)]).await?;
        }
    }

    let plan = deploy::plan_deploy(&cwd);
    println!("→ installing manifest ({})", app_id);
    client.install_app(&manifest).await.context("install_app failed")?;

    if plan.backend {
        println!("→ packaging backend/");
        let tar = archive::pack_dir(&cwd, Path::new("backend"))?;
        println!("→ uploading backend ({} bytes)", tar.len());
        client.deploy_app(&app_id, tar).await.context("deploy_app failed")?;
    }

    if plan.frontend {
        println!("→ packaging dist/");
        let tar = archive::pack_dir(&cwd, Path::new("dist"))?;
        println!("→ uploading frontend ({} bytes)", tar.len());
        client.deploy_frontend(&app_id, tar).await.context("deploy_frontend failed")?;
    }

    if plan.backend {
        println!("→ starting worker");
        match client.start_worker(&app_id).await {
            Ok(msg) => println!("  {msg}"),
            Err(e) => eprintln!("  ⚠ worker start: {e}"),
        }
    }

    println!("✓ deployed {app_id}");
    Ok(())
}

#[cfg(test)]
mod cli_parse_tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Cmd, clap::Error> {
        let full: Vec<&str> = std::iter::once("rootcx").chain(args.iter().copied()).collect();
        Cli::try_parse_from(full).map(|c| c.cmd)
    }

    #[test]
    fn top_level_aliases_route_to_expected_variants() {
        let cases: &[(&[&str], fn(&Cmd) -> bool)] = &[
            (&["i"], |c| matches!(c, Cmd::Init)),
            (&["init"], |c| matches!(c, Cmd::Init)),
            (&["n", "myapp"], |c| matches!(c, Cmd::New { name } if name == "myapp")),
            (&["new", "myapp"], |c| matches!(c, Cmd::New { name } if name == "myapp")),
            (&["d"], |c| matches!(c, Cmd::Deploy)),
            (&["deploy"], |c| matches!(c, Cmd::Deploy)),
            (&["s"], |c| matches!(c, Cmd::Status { json: false })),
            (&["status", "--json"], |c| matches!(c, Cmd::Status { json: true })),
        ];
        for (args, check) in cases {
            let parsed = parse(args).unwrap_or_else(|e| panic!("parse {args:?} failed: {e}"));
            assert!(check(&parsed), "unexpected variant for {args:?}: {parsed:?}");
        }
    }

    #[test]
    fn subcommand_groups_route_correctly() {
        let cases: &[(&[&str], fn(&Cmd) -> bool)] = &[
            (&["auth", "login"], |c| matches!(c, Cmd::Auth(AuthCmd::Login { url: None, token: None }))),
            (&["auth", "login", "http://x"], |c| matches!(c, Cmd::Auth(AuthCmd::Login { url: Some(u), .. }) if u == "http://x")),
            (&["auth", "login", "--token", "abc"], |c| matches!(c, Cmd::Auth(AuthCmd::Login { token: Some(t), .. }) if t == "abc")),
            (&["auth", "logout"], |c| matches!(c, Cmd::Auth(AuthCmd::Logout))),
            (&["auth", "whoami"], |c| matches!(c, Cmd::Auth(AuthCmd::Whoami { json: false }))),
            (&["auth", "whoami", "--json"], |c| matches!(c, Cmd::Auth(AuthCmd::Whoami { json: true }))),
            (&["apps", "list"], |c| matches!(c, Cmd::Apps(AppsCmd::List { json: false }))),
            (&["apps", "ls"], |c| matches!(c, Cmd::Apps(AppsCmd::List { .. }))),
            (&["apps", "rm", "foo"], |c| matches!(c, Cmd::Apps(AppsCmd::Rm { app_id, yes: false }) if app_id == "foo")),
            (&["apps", "rm", "foo", "-y"], |c| matches!(c, Cmd::Apps(AppsCmd::Rm { yes: true, .. }))),
            (&["agents", "list"], |c| matches!(c, Cmd::Agents(AgentsCmd::List { .. }))),
            (&["agents", "ls"], |c| matches!(c, Cmd::Agents(AgentsCmd::List { .. }))),
            (&["agents", "invoke", "a", "hi"], |c| matches!(c, Cmd::Agents(AgentsCmd::Invoke { app_id, message, session: None }) if app_id == "a" && message == "hi")),
            (&["agents", "invoke", "a", "hi", "--session", "s1"], |c| matches!(c, Cmd::Agents(AgentsCmd::Invoke { session: Some(s), .. }) if s == "s1")),
            (&["agents", "sessions", "a"], |c| matches!(c, Cmd::Agents(AgentsCmd::Sessions { app_id, .. }) if app_id == "a")),
        ];
        for (args, check) in cases {
            let parsed = parse(args).unwrap_or_else(|e| panic!("parse {args:?} failed: {e}"));
            assert!(check(&parsed), "unexpected variant for {args:?}: {parsed:?}");
        }
    }

    #[test]
    fn hidden_skills_path_is_still_invokable() {
        assert!(matches!(parse(&["skills-path"]).unwrap(), Cmd::SkillsPath));
    }

    #[test]
    fn completions_requires_valid_shell() {
        assert!(parse(&["completions", "bash"]).is_ok());
        assert!(parse(&["completions", "zsh"]).is_ok());
        assert!(parse(&["completions"]).is_err(), "shell argument should be required");
        assert!(parse(&["completions", "tcsh"]).is_err(), "unknown shell must be rejected");
    }

    #[test]
    fn required_positionals_are_enforced() {
        assert!(parse(&["new"]).is_err(), "`new` requires a name");
        assert!(parse(&["apps", "rm"]).is_err(), "`apps rm` requires an app_id");
        assert!(parse(&["agents", "invoke", "a"]).is_err(), "`agents invoke` requires message");
        assert!(parse(&["agents", "sessions"]).is_err(), "`agents sessions` requires app_id");
    }

    #[test]
    fn unknown_commands_are_rejected() {
        assert!(parse(&["bogus"]).is_err());
        assert!(parse(&["apps", "nope"]).is_err());
    }
}
