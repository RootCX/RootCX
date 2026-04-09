import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { RuntimeProvider } from "@rootcx/sdk";
import { ThemeProvider } from "@rootcx/ui";
import "./globals.css";
import App from "./App";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <RuntimeProvider>
      <ThemeProvider>
        <App />
      </ThemeProvider>
    </RuntimeProvider>
  </StrictMode>,
);
