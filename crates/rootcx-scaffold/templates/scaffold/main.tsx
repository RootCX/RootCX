import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { BrowserRouter } from "react-router-dom";
import { RuntimeProvider } from "@rootcx/sdk";
import { TooltipProvider, Toaster } from "@rootcx/ui";
import "./globals.css";
import App from "./App";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <BrowserRouter basename={import.meta.env.BASE_URL}>
      <RuntimeProvider>
        <TooltipProvider>
          <App />
          <Toaster />
        </TooltipProvider>
      </RuntimeProvider>
    </BrowserRouter>
  </StrictMode>,
);
