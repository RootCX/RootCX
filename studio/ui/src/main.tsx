import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { activateBuiltins } from "./extensions/activate";
import { installGlobalListener } from "./core/keybindings";
import { executeCommand } from "./core/studio";
import App from "./App.tsx";
import "./globals.css";

activateBuiltins();
installGlobalListener((id) => executeCommand(id));

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
