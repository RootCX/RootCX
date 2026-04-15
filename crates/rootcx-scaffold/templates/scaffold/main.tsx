import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { BrowserRouter } from "react-router-dom";
import { RuntimeProvider } from "@rootcx/sdk";
import { ThemeProvider } from "@rootcx/ui";
import "./globals.css";
import App from "./App";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <BrowserRouter>
      <RuntimeProvider>
        <ThemeProvider>
          <App />
        </ThemeProvider>
      </RuntimeProvider>
    </BrowserRouter>
  </StrictMode>,
);
