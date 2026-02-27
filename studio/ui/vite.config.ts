import path from "node:path";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// https://vite.dev/config/
export default defineConfig({
  plugins: [react(), tailwindcss()],

  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
      "@rootcx/ui": path.resolve(__dirname, "../../runtime/ui/src"),
    },
  },

  // Tauri expects a fixed port during development.
  server: {
    port: 1420,
    strictPort: true,
  },

  // Tauri uses a custom protocol in production; relative paths required.
  build: {
    outDir: "dist",
  },
});
