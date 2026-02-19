use crate::scaffold::emitter::Emitter;
use crate::scaffold::types::{AnswerValue, Layer, ScaffoldContext};
use std::future::Future;
use std::pin::Pin;

const GLOBALS_CSS: &str = include_str!("../../../templates/globals.css");

/// Emits: package.json, index.html, vite.config.ts, tsconfig.json,
/// src/main.tsx, src/globals.css, src/lib/utils.ts, components.json,
/// manifest.json, .rootcx/launch.json
pub struct CoreLayer;

impl Layer for CoreLayer {
    fn emit<'a>(&'a self, ctx: &'a ScaffoldContext, e: &'a Emitter) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        Box::pin(async move {
            let ScaffoldContext { name, app_id, port, sdk_path, .. } = ctx;
            let sdk_dep = format!("file:{}", sdk_path.display());

            // manifest.json
            let permissions = matches!(ctx.answers.get("permissions"), Some(AnswerValue::Bool(true)));
            let mut manifest = serde_json::json!({
                "appId": app_id, "name": name, "version": "0.0.1",
                "description": "", "dataContract": []
            });
            if permissions {
                manifest["permissions"] = serde_json::json!({
                    "roles": {
                        "admin": { "description": "Full access" },
                        "member": { "description": "Standard user", "inherits": [] }
                    },
                    "defaultRole": "member",
                    "policies": []
                });
            }
            e.write_json("manifest.json", &manifest).await?;
            e.write(".rootcx/launch.json", "{\n  \"command\": \"cargo tauri dev\"\n}\n").await?;

            // package.json
            e.write("package.json", &format!(r#"{{
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

            // index.html
            e.write("index.html", &format!(r#"<!DOCTYPE html>
<html lang="en">
<head><meta charset="UTF-8" /><meta name="viewport" content="width=device-width, initial-scale=1.0" /><title>{name}</title></head>
<body><div id="root"></div><script type="module" src="/src/main.tsx"></script></body>
</html>
"#)).await?;

            // vite.config.ts
            e.write("vite.config.ts", &format!(r#"import path from "path";
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

            // tsconfig.json
            e.write("tsconfig.json", r#"{
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

            // src/main.tsx
            e.write("src/main.tsx", r#"import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "./globals.css";
import App from "./App";

createRoot(document.getElementById("root")!).render(<StrictMode><App /></StrictMode>);
"#).await?;

            e.write("src/globals.css", GLOBALS_CSS).await?;

            // src/lib/utils.ts
            e.write("src/lib/utils.ts", r#"import { type ClassValue, clsx } from "clsx";
import { twMerge } from "tailwind-merge";

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}
"#).await?;

            // components.json (shadcn config)
            e.write("components.json", r#"{
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

            Ok(())
        })
    }
}
