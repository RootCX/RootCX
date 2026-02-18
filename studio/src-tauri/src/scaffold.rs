use std::hash::{Hash, Hasher};
use std::path::Path;
use tokio::fs;

const ICON: &[u8] = include_bytes!("../icons/32x32.png");
const GLOBALS_CSS: &str = include_str!("../templates/globals.css");

async fn w(path: &Path, content: &str) -> Result<(), String> {
    fs::write(path, content).await.map_err(|e| format!("write {}: {e}", path.display()))
}

pub async fn create(root: &Path, name: &str, sdk_path: &Path, client_crate_path: &Path) -> Result<(), String> {
    let app_id = name.to_lowercase().replace(' ', "-");
    let lib_name = app_id.replace('-', "_");
    let identifier = format!("com.rootcx.{app_id}");
    let mut h = std::hash::DefaultHasher::new();
    name.hash(&mut h);
    let port = 3000 + (h.finish() % 6000) as u16;

    for dir in ["src", "src/lib", "src-tauri/src", "src-tauri/capabilities", "src-tauri/icons", ".rootcx"] {
        fs::create_dir_all(root.join(dir)).await.map_err(|e| e.to_string())?;
    }
    fs::write(root.join("src-tauri/icons/icon.png"), ICON).await.map_err(|e| e.to_string())?;

    let manifest = serde_json::json!({
        "appId": app_id, "name": name, "version": "0.0.1",
        "description": "", "dataContract": []
    });
    w(&root.join("manifest.json"), &serde_json::to_string_pretty(&manifest).unwrap()).await?;
    w(&root.join(".rootcx/launch.json"), "{\n  \"command\": \"cargo tauri dev\"\n}\n").await?;

    let sdk_dep = format!("file:{}", sdk_path.display());
    w(&root.join("package.json"), &format!(r#"{{
  "name": "{app_id}",
  "private": true,
  "type": "module",
  "scripts": {{ "dev": "vite", "build": "vite build", "tauri": "tauri" }},
  "dependencies": {{
    "@rootcx/runtime": "{sdk_dep}",
    "@tailwindcss/vite": "^4.0.0",
    "class-variance-authority": "^0.7.0",
    "clsx": "^2.1.0",
    "@tabler/icons-react": "^3.30.0",
    "react": "^19.0.0",
    "react-dom": "^19.0.0",
    "tailwind-merge": "^2.5.0",
    "tailwindcss": "^4.0.0",
    "tw-animate-css": "^1.4.0"
  }},
  "devDependencies": {{
    "@tauri-apps/cli": "^2.0.0",
    "@types/node": "^22.0.0",
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

    w(&root.join("vite.config.ts"), &format!(r#"import path from "path";
import {{ defineConfig }} from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

export default defineConfig({{
  plugins: [react(), tailwindcss()],
  clearScreen: false,
  server: {{ port: {port}, strictPort: true }},
  resolve: {{
    alias: {{
      "@": path.resolve(__dirname, "./src"),
    }},
  }},
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
    "skipLibCheck": true,
    "baseUrl": ".",
    "paths": {
      "@/*": ["./src/*"]
    }
  },
  "include": ["src"]
}
"#).await?;

    w(&root.join("src/main.tsx"), r#"import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "./globals.css";
import App from "./App";

createRoot(document.getElementById("root")!).render(<StrictMode><App /></StrictMode>);
"#).await?;

    w(&root.join("src/App.tsx"), &format!(r#"export default function App() {{
  return <h1>{name}</h1>;
}}
"#)).await?;

    let client_crate_dep = client_crate_path.display();
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
reqwest = {{ version = "0.12", default-features = false, features = ["rustls-tls", "blocking"] }}
rootcx-runtime-client = {{ path = "{client_crate_dep}" }}
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
  "bundle": {{ "active": true, "icon": ["icons/icon.png"] }}
}}
"#)).await?;

    w(&root.join("src-tauri/capabilities/default.json"), r#"{
  "identifier": "default",
  "description": "Default capabilities",
  "windows": ["main"],
  "permissions": ["core:default"]
}
"#).await?;

    w(&root.join("src-tauri/src/lib.rs"), r#"pub fn run() {
    rootcx_runtime_client::ensure_runtime().expect("failed to start RootCX Runtime");

    let _ = reqwest::blocking::Client::new()
        .post("http://localhost:9100/api/v1/apps")
        .header("Content-Type", "application/json")
        .body(include_str!("../../manifest.json"))
        .send();

    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("error while running application");
}
"#).await?;

    w(&root.join("src-tauri/src/main.rs"), &format!(r#"#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {{
    {lib_name}_lib::run();
}}
"#)).await?;

    w(&root.join("src/globals.css"), GLOBALS_CSS).await?;

    w(&root.join("src/lib/utils.ts"), r#"import { type ClassValue, clsx } from "clsx";
import { twMerge } from "tailwind-merge";

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}
"#).await?;

    w(&root.join("components.json"), r#"{
  "$schema": "https://ui.shadcn.com/schema.json",
  "style": "radix-nova",
  "rsc": false,
  "tsx": true,
  "tailwind": {
    "config": "",
    "css": "src/globals.css",
    "baseColor": "neutral",
    "cssVariables": true
  },
  "iconLibrary": "tabler",
  "aliases": {
    "components": "@/components",
    "utils": "@/lib/utils",
    "ui": "@/components/ui",
    "lib": "@/lib",
    "hooks": "@/hooks"
  }
}
"#).await?;

    // ── Backend worker template ──────────────────────────────
    fs::create_dir_all(root.join("backend")).await.map_err(|e| e.to_string())?;
    w(&root.join("backend/index.ts"), r#"/**
 * RootCX Backend Worker
 *
 * Communicates with the runtime over stdin/stdout using JSON-line IPC.
 * Protocol:
 *   - Runtime sends: { "type": "discover" }  →  Worker replies with capabilities
 *   - Runtime sends: { "type": "rpc", "id": "...", "method": "...", "params": {...} }
 *   - Worker replies: { "type": "rpc_response", "id": "...", "result": ... }
 *   - Runtime sends: { "type": "job", "id": "...", "payload": {...} }
 *   - Worker replies: { "type": "job_result", "id": "...", "result": ... }
 *   - Runtime sends: { "type": "shutdown" }
 */

const send = (msg: Record<string, unknown>) => {
  process.stdout.write(JSON.stringify(msg) + "\n");
};

const handlers: Record<string, (params: any) => any> = {
  ping: () => ({ pong: true }),
  echo: (params: any) => params,
};

process.stdin.setEncoding("utf-8");

let buffer = "";
process.stdin.on("data", (chunk: string) => {
  buffer += chunk;
  let newline: number;
  while ((newline = buffer.indexOf("\n")) !== -1) {
    const line = buffer.slice(0, newline).trim();
    buffer = buffer.slice(newline + 1);
    if (!line) continue;

    try {
      const msg = JSON.parse(line);

      switch (msg.type) {
        case "discover":
          send({ type: "discover_result", methods: Object.keys(handlers) });
          break;

        case "rpc": {
          const fn = handlers[msg.method];
          if (fn) {
            send({ type: "rpc_response", id: msg.id, result: fn(msg.params) });
          } else {
            send({ type: "rpc_response", id: msg.id, error: `unknown method: ${msg.method}` });
          }
          break;
        }

        case "job":
          send({ type: "job_result", id: msg.id, result: { ok: true } });
          break;

        case "shutdown":
          process.exit(0);
      }
    } catch (e) {
      send({ type: "log", level: "error", message: `parse error: ${e}` });
    }
  }
});
"#).await?;

    Ok(())
}

