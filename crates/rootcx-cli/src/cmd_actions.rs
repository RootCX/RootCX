use std::io::{self, IsTerminal};

use anyhow::{Context, Result, bail, ensure};
use clap::{Args, Subcommand};
use rootcx_client::{ActionApprovalRequest, ActionApprovals, RuntimeClient};

use crate::{client_from_config, theme};

#[derive(Debug, Subcommand)]
#[command(after_help = "\
Review:       rootcx apps actions list <app> --json
Interactive:  rootcx apps actions approve <app> <action>
Automation:   rootcx apps actions approve <app> <action> --yes --revision <revision> --digest <backendDigest> --installation <installationId>

Approval applies only to the reviewed installation, revision, and backend code.")]
pub enum ActionsCmd {
    /// Show exact access, code digest, revision, installation, and approval status
    #[command(alias = "ls", alias = "inspect")]
    List {
        app_id: String,
        /// Emit the complete review snapshot as JSON
        #[arg(long)]
        json: bool,
    },
    /// Approve one action's reviewed access and code (admin only)
    Approve {
        app_id: String,
        action_id: String,
        #[command(flatten)]
        review: Review,
    },
    /// Revoke one action's approval (admin only)
    Revoke {
        app_id: String,
        action_id: String,
        /// Skip confirmation
        #[arg(short = 'y', long)]
        yes: bool,
    },
}

#[derive(Debug, Default, Args)]
pub struct Review {
    /// Require this exact reviewed revision
    #[arg(long)]
    revision: Option<String>,
    /// Require this exact reviewed backend digest
    #[arg(long)]
    digest: Option<String>,
    /// Require this exact reviewed installation ID
    #[arg(long)]
    installation: Option<String>,
    /// Skip confirmation; requires --revision, --digest, and --installation
    #[arg(short = 'y', long, requires_all = ["revision", "digest", "installation"])]
    yes: bool,
}

pub async fn run(cmd: ActionsCmd) -> Result<()> {
    match cmd {
        ActionsCmd::List { app_id, json } => {
            let client = client_from_config().await?;
            let snapshot = client
                .list_action_approvals(&app_id)
                .await
                .context("list action approvals failed")?;
            if json {
                println!("{}", serde_json::to_string_pretty(&snapshot)?);
            } else {
                println!("{}", preview(&app_id, &snapshot)?);
            }
        }
        ActionsCmd::Approve {
            app_id,
            action_id,
            review,
        } => {
            require_confirmation_mode(
                review.yes,
                io::stdin().is_terminal() && io::stdout().is_terminal(),
            )?;
            let client = client_from_config().await?;
            let approved = approve(&client, &app_id, &action_id, &review, || {
                confirm(&format!(
                    "Approve '{app_id}/{action_id}' with exactly this access and code?"
                ))
            })
            .await?;
            if approved {
                println!("✓ approved {app_id}/{action_id}");
            } else {
                println!("Cancelled");
            }
        }
        ActionsCmd::Revoke {
            app_id,
            action_id,
            yes,
        } => {
            require_confirmation_mode(
                yes,
                io::stdin().is_terminal() && io::stdout().is_terminal(),
            )?;
            if !yes && !confirm(&format!("Revoke approval for '{app_id}/{action_id}'?"))? {
                println!("Cancelled");
                return Ok(());
            }
            let client = client_from_config().await?;
            client
                .revoke_action_approval(&app_id, &action_id)
                .await
                .context("revoke action approval failed")?;
            println!("✓ revoked {app_id}/{action_id}");
        }
    }
    Ok(())
}

fn require_confirmation_mode(yes: bool, terminal: bool) -> Result<()> {
    ensure!(
        yes || terminal,
        "confirmation requires a terminal; use --yes (approval also requires --revision, --digest, and --installation from `rootcx apps actions list <app> --json`)"
    );
    Ok(())
}

fn confirm(message: &str) -> Result<bool> {
    cliclack::set_theme(theme::RootcxTheme);
    Ok(cliclack::confirm(message).initial_value(false).interact()?)
}

async fn approve(
    client: &RuntimeClient,
    app_id: &str,
    action_id: &str,
    review: &Review,
    confirm: impl FnOnce() -> Result<bool>,
) -> Result<bool> {
    let mut snapshot = client
        .list_action_approvals(app_id)
        .await
        .context("list action approvals failed")?;
    let request = reviewed_request(&snapshot, action_id, review)?;
    snapshot.actions.retain(|action| action.id == action_id);
    println!("{}", preview(app_id, &snapshot)?);
    if !review.yes && !confirm()? {
        return Ok(false);
    }
    client
        .approve_action(app_id, action_id, &request)
        .await
        .context(
            "approval failed; review again if the revision, digest, or installation changed",
        )?;
    Ok(true)
}

fn reviewed_request(
    snapshot: &ActionApprovals,
    action_id: &str,
    review: &Review,
) -> Result<ActionApprovalRequest> {
    ensure!(
        snapshot.actions.iter().any(|action| action.id == action_id),
        "action '{action_id}' is not declared for approval"
    );
    let pin = |flag: &str, actual: Option<&str>, expected: Option<&str>| -> Result<String> {
        let actual = actual
            .filter(|value| !value.trim().is_empty())
            .with_context(|| format!("no {flag} available; action cannot be approved"))?;
        if let Some(expected) = expected {
            ensure!(
                expected == actual,
                "reviewed --{flag} does not match the current app; inspect action access again"
            );
        } else if review.yes {
            bail!("--yes requires --{flag} from the reviewed snapshot");
        }
        Ok(actual.to_owned())
    };
    Ok(ActionApprovalRequest {
        revision: pin(
            "revision",
            snapshot.revision.as_deref(),
            review.revision.as_deref(),
        )?,
        backend_digest: pin(
            "digest",
            snapshot.backend_digest.as_deref(),
            review.digest.as_deref(),
        )?,
        installation_id: pin(
            "installation",
            Some(&snapshot.installation_id),
            review.installation.as_deref(),
        )?,
    })
}

fn preview(app_id: &str, snapshot: &ActionApprovals) -> Result<String> {
    Ok(format!(
        "App: {app_id}\nInstallation: {}\nRevision: {}\nBackend digest: {}\nActions:\n{}",
        snapshot.installation_id,
        snapshot.revision.as_deref().unwrap_or("(none)"),
        snapshot.backend_digest.as_deref().unwrap_or("(none)"),
        serde_json::to_string_pretty(&snapshot.actions)?,
    ))
}

#[cfg(test)]
mod tests;
