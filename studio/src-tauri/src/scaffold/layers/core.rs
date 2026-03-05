use crate::scaffold::emitter::Emitter;
use crate::scaffold::types::{AnswerValue, Layer, LayerFuture, ScaffoldContext};

const TPL_TSCONFIG: &str = include_str!("../../../templates/scaffold/tsconfig.json");
const TPL_MAIN_TSX: &str = include_str!("../../../templates/scaffold/main.tsx");
const TPL_GLOBALS_CSS: &str = include_str!("../../../templates/scaffold/globals.css");
const TPL_UTILS_TS: &str = include_str!("../../../templates/scaffold/utils.ts");

pub struct CoreLayer;

impl Layer for CoreLayer {
    fn emit<'a>(&'a self, ctx: &'a ScaffoldContext, e: &'a Emitter) -> LayerFuture<'a> {
        Box::pin(async move {
            let ScaffoldContext { app_id, port, .. } = ctx;

            let permissions = matches!(ctx.answers.get("permissions"), Some(AnswerValue::Bool(true)));
            let mut manifest = serde_json::json!({
                "appId": app_id, "name": app_id, "version": "0.0.1",
                "description": "", "dataContract": []
            });
            if permissions {
                manifest["permissions"] = serde_json::json!({
                    "permissions": []
                });
            }
            e.write_json("manifest.json", &manifest).await?;
            e.write(".rootcx/launch.json", "{\n  \"preLaunch\": [\"verify_schema\", \"sync_manifest\", \"deploy_backend\"],\n  \"command\": \"cargo tauri dev\"\n}\n").await?;

            e.write(
                "package.json",
                &format!(
                    r#"{{
  "name": "{app_id}",
  "private": true,
  "type": "module",
  "scripts": {{ "dev": "vite", "build": "vite build", "tauri": "tauri" }},
  "dependencies": {{
    "@rootcx/sdk": "~0.1.0",
    "@rootcx/ui": "~0.1.0",
    "@tabler/icons-react": "^3.30.0",
    "@tauri-apps/plugin-shell": "^2.0.0",
    "@tailwindcss/vite": "^4.0.0",
    "clsx": "^2.1.0",
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
"#
                ),
            )
            .await?;

            e.write("index.html", &format!(r#"<!DOCTYPE html>
<html lang="en">
<head><meta charset="UTF-8" /><meta name="viewport" content="width=device-width, initial-scale=1.0" /><title>{app_id}</title></head>
<body><div id="root"></div><script type="module" src="/src/main.tsx"></script></body>
</html>
"#)).await?;

            e.write(
                "vite.config.ts",
                &format!(
                    r#"import path from "path";
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
"#
                ),
            )
            .await?;

            e.write(".gitignore", "node_modules/\ndist/\ntarget/\n.bundle/\nsrc-tauri/vendor/\nsrc-tauri/resources/\n").await?;

            e.write("tsconfig.json", TPL_TSCONFIG).await?;
            e.write("src/main.tsx", TPL_MAIN_TSX).await?;
            e.write("src/globals.css", TPL_GLOBALS_CSS).await?;
            e.write("src/lib/utils.ts", TPL_UTILS_TS).await?;

            Ok(())
        })
    }
}
