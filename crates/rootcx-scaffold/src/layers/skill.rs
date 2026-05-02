use crate::emitter::Emitter;
use crate::types::{Layer, LayerFuture, ScaffoldContext};
use std::path::PathBuf;

/// Creates a `.claude/skills/rootcx` symlink pointing to the bundled skill directory,
/// and writes a minimal `skills-lock.json` at the project root.
pub struct SkillLayer {
    /// Absolute path to the project root (needed for direct fs ops).
    project_root: PathBuf,
    /// Absolute path to the bundled skill source (e.g. `~/.rootcx/skills/rootcx`).
    skills_source: PathBuf,
}

impl SkillLayer {
    pub fn new(project_root: PathBuf, skills_source: PathBuf) -> Self {
        Self { project_root, skills_source }
    }
}

impl Layer for SkillLayer {
    fn emit<'a>(&'a self, _ctx: &'a ScaffoldContext, e: &'a Emitter) -> LayerFuture<'a> {
        Box::pin(async move {
            // Create .claude/skills/ directory
            let skills_dir = self.project_root.join(".claude/skills");
            tokio::fs::create_dir_all(&skills_dir)
                .await
                .map_err(|err| format!("mkdir .claude/skills: {err}"))?;

            // Create symlink: .claude/skills/rootcx -> skills_source
            let link_path = skills_dir.join("rootcx");
            #[cfg(unix)]
            tokio::fs::symlink(&self.skills_source, &link_path)
                .await
                .map_err(|err| format!("symlink .claude/skills/rootcx -> {}: {err}", self.skills_source.display()))?;

            #[cfg(windows)]
            tokio::fs::symlink_dir(&self.skills_source, &link_path)
                .await
                .map_err(|err| format!("symlink .claude/skills/rootcx -> {}: {err}", self.skills_source.display()))?;

            // Write a minimal skills-lock.json for compat with the `skills` CLI tool
            let lock = serde_json::json!({
                "skills": {
                    "rootcx": {
                        "source": self.skills_source.to_string_lossy(),
                        "type": "symlink"
                    }
                }
            });
            e.write_json("skills-lock.json", &lock).await?;

            Ok(())
        })
    }
}
