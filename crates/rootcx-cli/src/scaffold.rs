use anyhow::{Result, bail};
use std::path::Path;

const TPL_TSCONFIG: &str = include_str!("../../../studio/src-tauri/templates/scaffold/tsconfig.json");
const TPL_MAIN_TSX: &str = include_str!("../../../studio/src-tauri/templates/scaffold/main.tsx");
const TPL_GLOBALS_CSS: &str = include_str!("../../../studio/src-tauri/templates/scaffold/globals.css");
const TPL_UTILS_TS: &str = include_str!("../../../studio/src-tauri/templates/scaffold/utils.ts");
const TPL_AGENT_APP: &str = include_str!("../../../studio/src-tauri/templates/scaffold/agent-app.tsx");
const TPL_AGENT_INDEX: &str = include_str!("../../../studio/src-tauri/templates/scaffold/agent/index.ts");

pub fn run(kind: ProjectKind, name: &str, base: &Path) -> Result<()> {
    let dir = base.join(name);
    if dir.exists() {
        bail!("{} already exists", name);
    }
    core_layer(&dir, name)?;
    match kind {
        ProjectKind::Agent => agent_layer(&dir, name)?,
        ProjectKind::App => write(dir.join("src/App.tsx"), &app_stub(name))?,
    }
    println!("✓ scaffolded {kind} {name}");
    println!("  next: cd {name} && bun install && bun run build && rootcx deploy");
    Ok(())
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum ProjectKind {
    App,
    Agent,
}

impl std::fmt::Display for ProjectKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::App => f.write_str("app"),
            Self::Agent => f.write_str("agent"),
        }
    }
}

fn core_layer(dir: &Path, name: &str) -> Result<()> {
    write(dir.join("manifest.json"), &manifest_json(name))?;
    write(dir.join(".rootcx/launch.json"), "{\n  \"preLaunch\": [\"verify_schema\", \"sync_manifest\", \"install_deps\", \"deploy_backend\", \"publish_frontend\"]\n}\n")?;
    write(dir.join("package.json"), &package_json(name))?;
    write(dir.join("index.html"), &index_html(name))?;
    write(dir.join("vite.config.ts"), VITE_CONFIG)?;
    write(dir.join(".gitignore"), ".rootcx/\nnode_modules/\ndist/\n")?;
    write(dir.join("tsconfig.json"), TPL_TSCONFIG)?;
    write(dir.join("src/main.tsx"), TPL_MAIN_TSX)?;
    write(dir.join("src/globals.css"), TPL_GLOBALS_CSS)?;
    write(dir.join("src/lib/utils.ts"), TPL_UTILS_TS)?;
    Ok(())
}

fn agent_layer(dir: &Path, name: &str) -> Result<()> {
    write(dir.join("backend/agent.json"), &agent_json(name))?;
    write(dir.join("backend/agent/system.md"), &format!(
        "You are the {name} agent.\n\n## Your role\nDescribe what this agent does.\n\n## Tools\nYou have access to tools for querying and mutating data, searching the web, and fetching pages.\nUse them as needed to fulfill user requests.\n"
    ))?;
    let index = TPL_AGENT_INDEX
        .replace("__LLM_IMPORT__", r#"import { ChatAnthropic } from "@langchain/anthropic";"#)
        .replace(
            "__LLM_INIT__",
            r#"new ChatAnthropic({ apiKey: credentials.ANTHROPIC_API_KEY, model: "claude-sonnet-4-6" })"#,
        );
    write(dir.join("backend/index.ts"), &index)?;
    write(dir.join("backend/package.json"), &agent_backend_pkg(name))?;
    write(dir.join("src/App.tsx"), &TPL_AGENT_APP.replace("__APP_ID__", name))?;
    Ok(())
}

fn write(path: impl AsRef<Path>, content: &str) -> Result<()> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content)?;
    Ok(())
}

fn manifest_json(name: &str) -> String {
    format!(
        r#"{{
  "appId": "{name}",
  "name": "{name}",
  "version": "0.0.1",
  "description": "",
  "dataContract": [],
  "permissions": {{ "permissions": [] }}
}}
"#
    )
}

fn package_json(name: &str) -> String {
    format!(
        r#"{{
  "name": "{name}",
  "private": true,
  "type": "module",
  "scripts": {{ "dev": "vite", "build": "vite build" }},
  "dependencies": {{
    "@rootcx/sdk": "^0.10.0",
    "@rootcx/ui": "^0.5.0",
    "@tabler/icons-react": "^3.30.0",
    "@tailwindcss/vite": "^4.0.0",
    "clsx": "^2.1.0",
    "react": "^19.0.0",
    "react-dom": "^19.0.0",
    "tailwind-merge": "^2.5.0",
    "tailwindcss": "^4.0.0",
    "tw-animate-css": "^1.4.0"
  }},
  "devDependencies": {{
    "@types/node": "^22.0.0",
    "@types/react": "^19.0.0",
    "@types/react-dom": "^19.0.0",
    "@vitejs/plugin-react": "^4.0.0",
    "typescript": "^5.7.0",
    "vite": "^6.0.0"
  }}
}}
"#
    )
}

fn index_html(name: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head><meta charset="UTF-8" /><meta name="viewport" content="width=device-width, initial-scale=1.0" /><title>{name}</title></head>
<body><div id="root"></div><script type="module" src="/src/main.tsx"></script></body>
</html>
"#
    )
}

const VITE_CONFIG: &str = r#"import path from "path";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

export default defineConfig({
  base: "./",
  plugins: [react(), tailwindcss()],
  clearScreen: false,
  resolve: {
    alias: { "@": path.resolve(__dirname, "./src") },
  },
});
"#;

fn app_stub(name: &str) -> String {
    format!(
        r#"import {{ AuthGate }} from "@rootcx/sdk";
import {{ AppShell, AppShellSidebar, AppShellMain, Sidebar, SidebarItem, PageHeader, Button }} from "@rootcx/ui";
import {{ IconLogout, IconHome }} from "@tabler/icons-react";

export default function App() {{
  return (
    <AuthGate appTitle="{name}">
      {{({{ user, logout }}) => (
        <AppShell>
          <AppShellSidebar>
            <Sidebar
              header={{<span className="text-sm font-semibold">{name}</span>}}
              footer={{
                <div className="flex items-center justify-between">
                  <span className="truncate text-sm text-muted-foreground">{{user.email}}</span>
                  <Button variant="ghost" size="icon" onClick={{() => logout()}}>
                    <IconLogout className="h-4 w-4" />
                  </Button>
                </div>
              }}
            >
              <SidebarItem icon={{<IconHome />}} label="Home" active />
            </Sidebar>
          </AppShellSidebar>
          <AppShellMain>
            <div className="p-6">
              <PageHeader title="Home" description="Welcome to {name}" />
            </div>
          </AppShellMain>
        </AppShell>
      )}}
    </AuthGate>
  );
}}
"#
    )
}

fn agent_json(name: &str) -> String {
    format!(
        r#"{{
  "name": "{name}",
  "description": "AI agent for {name}",
  "systemPrompt": "./agent/system.md",
  "memory": {{ "enabled": true }},
  "limits": {{ "maxTurns": 50, "maxContextTokens": 100000, "keepRecentMessages": 10 }},
  "supervision": {{ "mode": "autonomous" }}
}}
"#
    )
}

fn agent_backend_pkg(name: &str) -> String {
    format!(
        r#"{{
  "name": "{name}-backend",
  "version": "0.1.0",
  "private": true,
  "type": "module",
  "dependencies": {{
    "langchain": "^1.2.0",
    "@langchain/core": "^1.1.0",
    "@langchain/anthropic": "^1.3.0",
    "zod": "^3.25.0"
  }}
}}
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn manifest_json_is_valid_and_carries_name() {
        let v: Value = serde_json::from_str(&manifest_json("foo")).unwrap();
        assert_eq!(v["appId"], "foo");
        assert_eq!(v["name"], "foo");
        assert!(v["dataContract"].is_array());
    }

    #[test]
    fn package_json_is_valid_and_has_build_script() {
        let v: Value = serde_json::from_str(&package_json("foo")).unwrap();
        assert_eq!(v["name"], "foo");
        assert_eq!(v["scripts"]["build"], "vite build");
        assert!(v["dependencies"]["@rootcx/sdk"].is_string());
        assert!(v["devDependencies"]["vite"].is_string());
    }

    #[test]
    fn agent_json_is_valid() {
        let v: Value = serde_json::from_str(&agent_json("foo")).unwrap();
        assert_eq!(v["name"], "foo");
        assert_eq!(v["supervision"]["mode"], "autonomous");
    }

    #[test]
    fn agent_backend_pkg_is_valid_json() {
        let v: Value = serde_json::from_str(&agent_backend_pkg("foo")).unwrap();
        assert_eq!(v["name"], "foo-backend");
        assert!(v["dependencies"]["@langchain/anthropic"].is_string());
    }

    /// Regression: deployed apps are served under a subpath (`/apps/{id}/` or
    /// `/{id}/` via cloud). Absolute `/assets/...` 404s. `base: "./"` fixes it.
    #[test]
    fn vite_config_uses_relative_base() {
        assert!(VITE_CONFIG.contains(r#"base: "./""#), "missing base: './' — assets will 404 under subpath");
    }

    #[test]
    fn run_bails_when_target_exists() {
        let root = crate::testutil::scratch("scaffold-exists");
        std::fs::create_dir_all(root.join("foo")).unwrap();
        let err = run(ProjectKind::App, "foo", &root).unwrap_err().to_string();
        assert!(err.contains("already exists"));
    }

    #[test]
    fn run_app_writes_full_buildable_layout() {
        let root = crate::testutil::scratch("scaffold-app");
        run(ProjectKind::App, "myapp", &root).unwrap();
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
        ] {
            assert!(app.join(f).exists(), "missing {f}");
        }
        assert!(!app.join("backend").exists());
        let manifest = std::fs::read_to_string(app.join("manifest.json")).unwrap();
        assert!(manifest.contains("\"appId\": \"myapp\""));
        let app_tsx = std::fs::read_to_string(app.join("src/App.tsx")).unwrap();
        assert!(app_tsx.contains("AuthGate"));
        assert!(app_tsx.contains("user.email"));
        assert!(app_tsx.contains("PageHeader"));
    }

    #[test]
    fn run_agent_writes_backend_and_substitutes_llm_placeholders() {
        let root = crate::testutil::scratch("scaffold-agent");
        run(ProjectKind::Agent, "bot", &root).unwrap();
        let app = root.join("bot");
        for f in [
            "backend/agent.json",
            "backend/agent/system.md",
            "backend/index.ts",
            "backend/package.json",
            "src/App.tsx",
        ] {
            assert!(app.join(f).exists(), "missing {f}");
        }
        let index = std::fs::read_to_string(app.join("backend/index.ts")).unwrap();
        assert!(!index.contains("__LLM_IMPORT__"), "placeholder not substituted");
        assert!(!index.contains("__LLM_INIT__"), "placeholder not substituted");
        assert!(index.contains("ChatAnthropic"));

        let app_tsx = std::fs::read_to_string(app.join("src/App.tsx")).unwrap();
        assert!(!app_tsx.contains("__APP_ID__"));
        assert!(app_tsx.contains("bot"));

        let sys = std::fs::read_to_string(app.join("backend/agent/system.md")).unwrap();
        assert!(sys.contains("bot"));
    }
}
