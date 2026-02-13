import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// https://vite.dev/config/
export default defineConfig({
  plugins: [react()],

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
