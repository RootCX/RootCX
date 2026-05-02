use anyhow::{Result, bail};
use std::path::Path;

use crate::config;

const SKILLS_REPO: &str = "https://github.com/RootCX/skills.git";

pub async fn run(name: &str, base: &Path) -> Result<()> {
    let dir = base.join(name);
    if dir.exists() {
        bail!("{} already exists", name);
    }

    let registry = rootcx_scaffold::Registry::new();
    let preset = registry.get("blank").map_err(|e| anyhow::anyhow!(e))?;
    let answers = preset.questions()
        .into_iter()
        .filter_map(|q| q.default.map(|d| (q.key, d)))
        .collect();

    let skills_source = ensure_skills().await?;
    let extra_layers: Vec<Box<dyn rootcx_scaffold::types::Layer>> = vec![
        Box::new(rootcx_scaffold::layers::SkillLayer::new(dir.clone(), skills_source)),
    ];

    rootcx_scaffold::create(&dir, name, "blank", answers, extra_layers)
        .await
        .map_err(|e| anyhow::anyhow!(e))?;

    println!("\u{2713} created {name}");
    println!("  cd {name} && rootcx deploy");
    Ok(())
}

/// Ensures skills repo is cloned to ~/.rootcx/skills. Returns the repo root.
/// Does NOT pull — callers decide whether to pull (background or foreground).
pub async fn ensure_skills_cloned() -> Result<std::path::PathBuf> {
    let skills_dir = config::skills_dir()?;
    let rootcx_skill = skills_dir.join("rootcx");

    if !rootcx_skill.join("SKILL.md").exists() {
        if skills_dir.exists() {
            std::fs::remove_dir_all(&skills_dir)?;
        }
        std::fs::create_dir_all(skills_dir.parent().unwrap_or(&skills_dir))?;
        let status = tokio::process::Command::new("git")
            .args(["clone", "--depth", "1", "-q", SKILLS_REPO])
            .arg(&skills_dir)
            .status()
            .await?;
        if !status.success() {
            bail!("failed to clone skills repository. Make sure git is installed.");
        }
    }

    Ok(skills_dir)
}

/// Ensures skills are cloned, then fires a best-effort background pull.
/// Returns the skill subdirectory path (~/.rootcx/skills/rootcx).
pub async fn ensure_skills() -> Result<std::path::PathBuf> {
    let skills_dir = ensure_skills_cloned().await?;

    let dir = skills_dir.clone();
    tokio::spawn(async move {
        tokio::process::Command::new("git")
            .args(["pull", "--ff-only", "-q"])
            .current_dir(&dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await
            .ok();
    });

    Ok(skills_dir.join("rootcx"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn run_bails_when_target_exists() {
        let root = crate::testutil::scratch("scaffold-exists");
        std::fs::create_dir_all(root.join("foo")).unwrap();
        let err = run("foo", &root).await.unwrap_err().to_string();
        assert!(err.contains("already exists"));
    }

    #[tokio::test]
    async fn run_writes_full_buildable_layout() {
        let root = crate::testutil::scratch("scaffold-app");
        run("myapp", &root).await.unwrap();
        let app = root.join("myapp");
        for f in [
            "manifest.json",
            "package.json",
            "index.html",
            "vite.config.ts",
            "tsconfig.json",
            ".gitignore",
            "src/main.tsx",
            "src/globals.css",
            "src/lib/utils.ts",
            "src/App.tsx",
            "backend/index.ts",
        ] {
            assert!(app.join(f).exists(), "missing {f}");
        }
        let manifest = std::fs::read_to_string(app.join("manifest.json")).unwrap();
        assert!(manifest.contains("\"appId\": \"myapp\""));
        let app_tsx = std::fs::read_to_string(app.join("src/App.tsx")).unwrap();
        assert!(app_tsx.contains("AuthGate"));
        let backend = std::fs::read_to_string(app.join("backend/index.ts")).unwrap();
        assert!(backend.contains("serve("));
    }
}
