import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { activateBuiltins } from "./extensions/builtin";
import App from "./App.tsx";
import "./globals.css";

activateBuiltins();

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
