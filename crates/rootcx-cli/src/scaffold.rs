use anyhow::{Result, bail};
use std::path::Path;

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

    rootcx_scaffold::create(&dir, name, "blank", answers, vec![])
        .await
        .map_err(|e| anyhow::anyhow!(e))?;

    println!("\u{2713} created {name}");
    println!("  cd {name} && rootcx deploy");
    Ok(())
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
