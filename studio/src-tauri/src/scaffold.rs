use std::hash::{Hash, Hasher};
use std::path::Path;
use tokio::fs;

const ICON: &[u8] = include_bytes!("../icons/32x32.png");

async fn w(path: &Path, content: &str) -> Result<(), String> {
    fs::write(path, content).await.map_err(|e| format!("write {}: {e}", path.display()))
}

pub async fn create(root: &Path, name: &str) -> Result<(), String> {
    let app_id = name.to_lowercase().replace(' ', "-");
    let lib_name = app_id.replace('-', "_");
    let identifier = format!("com.rootcx.{app_id}");
    let mut h = std::hash::DefaultHasher::new();
    name.hash(&mut h);
    let port = 3000 + (h.finish() % 6000) as u16;

    for dir in ["src", "src-tauri/src", "src-tauri/capabilities", "src-tauri/icons", ".rootcx"] {
        fs::create_dir_all(root.join(dir)).await.map_err(|e| e.to_string())?;
    }
    fs::write(root.join("src-tauri/icons/icon.png"), ICON).await.map_err(|e| e.to_string())?;

    let manifest = serde_json::json!({
        "appId": app_id, "name": name, "version": "0.0.1",
        "description": "", "dataContract": []
    });
    w(&root.join("manifest.json"), &serde_json::to_string_pretty(&manifest).unwrap()).await?;
    w(&root.join(".rootcx/launch.json"), "{\n  \"command\": \"cargo tauri dev\"\n}\n").await?;

    w(&root.join("package.json"), &format!(r#"{{
  "name": "{app_id}",
  "private": true,
  "type": "module",
  "scripts": {{ "dev": "vite", "build": "vite build", "tauri": "tauri" }},
  "dependencies": {{ "react": "^19.0.0", "react-dom": "^19.0.0" }},
  "devDependencies": {{
    "@tauri-apps/cli": "^2.0.0",
    "@types/react": "^19.0.0",
    "@types/react-dom": "^19.0.0",
    "@vitejs/plugin-react": "^4.0.0",
    "typescript": "^5.7.0",
    "vite": "^6.0.0"
  }}
}}
"#)).await?;

    w(&root.join("index.html"), &format!(r#"<!DOCTYPE html>
<html lang="en">
<head><meta charset="UTF-8" /><meta name="viewport" content="width=device-width, initial-scale=1.0" /><title>{name}</title></head>
<body><div id="root"></div><script type="module" src="/src/main.tsx"></script></body>
</html>
"#)).await?;

    w(&root.join("vite.config.ts"), &format!(r#"import {{ defineConfig }} from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({{
  plugins: [react()],
  clearScreen: false,
  server: {{ port: {port}, strictPort: true }},
}});
"#)).await?;

    w(&root.join("tsconfig.json"), r#"{
  "compilerOptions": {
    "target": "ES2020",
    "module": "ESNext",
    "moduleResolution": "bundler",
    "jsx": "react-jsx",
    "strict": true,
    "esModuleInterop": true,
    "skipLibCheck": true
  },
  "include": ["src"]
}
"#).await?;

    w(&root.join("src/main.tsx"), r#"import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";

createRoot(document.getElementById("root")!).render(<StrictMode><App /></StrictMode>);
"#).await?;

    w(&root.join("src/App.tsx"), &format!(r#"export default function App() {{
  return <h1>{name}</h1>;
}}
"#)).await?;

    w(&root.join("src-tauri/Cargo.toml"), &format!(r#"[package]
name = "{app_id}"
version = "0.0.1"
edition = "2021"

[lib]
name = "{lib_name}_lib"
crate-type = ["staticlib", "cdylib", "rlib"]

[build-dependencies]
tauri-build = {{ version = "2", features = [] }}

[dependencies]
tauri = {{ version = "2", features = [] }}
serde = {{ version = "1", features = ["derive"] }}
serde_json = "1"
"#)).await?;

    w(&root.join("src-tauri/build.rs"), "fn main() { tauri_build::build(); }\n").await?;

    w(&root.join("src-tauri/tauri.conf.json"), &format!(r#"{{
  "productName": "{name}",
  "version": "0.0.1",
  "identifier": "{identifier}",
  "build": {{
    "beforeDevCommand": "npm install && npm run dev",
    "devUrl": "http://localhost:{port}",
    "beforeBuildCommand": "npm run build",
    "frontendDist": "../dist"
  }},
  "app": {{
    "windows": [{{ "title": "{name}", "width": 900, "height": 600 }}],
    "security": {{ "csp": null }}
  }},
  "bundle": {{ "icon": ["icons/icon.png"] }}
}}
"#)).await?;

    w(&root.join("src-tauri/capabilities/default.json"), r#"{
  "identifier": "default",
  "description": "Default capabilities",
  "windows": ["main"],
  "permissions": ["core:default"]
}
"#).await?;

    w(&root.join("src-tauri/src/lib.rs"), r#"#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {name}!")
}

pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![greet])
        .run(tauri::generate_context!())
        .expect("error while running application");
}
"#).await?;

    w(&root.join("src-tauri/src/main.rs"), &format!(r#"#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {{
    {lib_name}_lib::run();
}}
"#)).await?;

    Ok(())
}
