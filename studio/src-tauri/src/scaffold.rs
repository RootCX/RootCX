use std::hash::{Hash, Hasher};
use std::path::Path;
use tokio::fs;

const ICON: &[u8] = include_bytes!("../icons/32x32.png");
const GLOBALS_CSS: &str = include_str!("../templates/globals.css");
const BUTTON_TSX: &str = include_str!("../templates/components/button.tsx");
const INPUT_TSX: &str = include_str!("../templates/components/input.tsx");
const LABEL_TSX: &str = include_str!("../templates/components/label.tsx");
const CARD_TSX: &str = include_str!("../templates/components/card.tsx");
const BACKEND_WORKER: &str = include_str!("../templates/backend-worker.ts");

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

    for dir in ["src", "src/lib", "src/components/ui", "src-tauri/src", "src-tauri/capabilities", "src-tauri/icons", "backend", ".rootcx"] {
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

    w(&root.join("src/App.tsx"), &format!(r##"import {{ useState }} from "react";
import {{ useAuth }} from "@rootcx/runtime";
import {{ Card, CardHeader, CardTitle, CardDescription, CardContent }} from "@/components/ui/card";
import {{ Button }} from "@/components/ui/button";
import {{ Input }} from "@/components/ui/input";
import {{ Label }} from "@/components/ui/label";

export default function App() {{
  const {{ user, loading, isAuthenticated, login, register, logout }} = useAuth();
  const [mode, setMode] = useState<"login" | "register">("login");
  const [error, setError] = useState<string | null>(null);

  if (loading) {{
    return (
      <div className="flex min-h-screen items-center justify-center">
        <p className="text-muted-foreground">Loading…</p>
      </div>
    );
  }}

  if (!isAuthenticated) {{
    return (
      <div className="flex min-h-screen items-center justify-center bg-background px-4">
        <Card className="w-full max-w-sm">
          <CardHeader>
            <CardTitle className="text-2xl">{name}</CardTitle>
            <CardDescription>
              {{mode === "login" ? "Sign in to your account" : "Create a new account"}}
            </CardDescription>
          </CardHeader>
          <CardContent>
            <form
              className="space-y-4"
              onSubmit={{async (e) => {{
                e.preventDefault();
                setError(null);
                const fd = new FormData(e.currentTarget);
                const username = fd.get("username") as string;
                const password = fd.get("password") as string;
                try {{
                  if (mode === "register") {{
                    await register({{ username, password }});
                    await login(username, password);
                  }} else {{
                    await login(username, password);
                  }}
                }} catch (err) {{
                  setError(err instanceof Error ? err.message : "Authentication failed");
                }}
              }}}}
            >
              {{error && <p className="text-sm text-destructive">{{error}}</p>}}
              <div className="space-y-2">
                <Label htmlFor="username">Username</Label>
                <Input id="username" name="username" placeholder="Username" required />
              </div>
              <div className="space-y-2">
                <Label htmlFor="password">Password</Label>
                <Input id="password" name="password" type="password" placeholder="Password" minLength={{8}} required />
              </div>
              <Button type="submit" className="w-full">
                {{mode === "login" ? "Sign in" : "Create account"}}
              </Button>
              <p className="text-center text-sm text-muted-foreground">
                {{mode === "login" ? "No account? " : "Already have one? "}}
                <button
                  type="button"
                  className="text-primary underline-offset-4 hover:underline"
                  onClick={{() => {{ setMode(mode === "login" ? "register" : "login"); setError(null); }}}}
                >
                  {{mode === "login" ? "Register" : "Sign in"}}
                </button>
              </p>
            </form>
          </CardContent>
        </Card>
      </div>
    );
  }}

  return (
    <div className="flex min-h-screen items-center justify-center bg-background px-4">
      <Card className="w-full max-w-sm">
        <CardHeader>
          <CardTitle className="text-2xl">{name}</CardTitle>
          <CardDescription>Signed in as {{user!.username}}</CardDescription>
        </CardHeader>
        <CardContent>
          <Button variant="outline" className="w-full" onClick={{() => logout()}}>Sign out</Button>
        </CardContent>
      </Card>
    </div>
  );
}}
"##)).await?;

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

    // ── shadcn/ui components ──────────────────────────────
    w(&root.join("src/components/ui/button.tsx"), BUTTON_TSX).await?;
    w(&root.join("src/components/ui/input.tsx"), INPUT_TSX).await?;
    w(&root.join("src/components/ui/label.tsx"), LABEL_TSX).await?;
    w(&root.join("src/components/ui/card.tsx"), CARD_TSX).await?;

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
    w(&root.join("backend/index.ts"), BACKEND_WORKER).await?;

    Ok(())
}

