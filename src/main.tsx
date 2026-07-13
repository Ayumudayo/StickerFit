import { StrictMode } from "react";
import ReactDOM from "react-dom/client";

import App from "./App";
import { installCspDiagnostics } from "./platform/cspDiagnostics";
import "./styles/app-base.css";
import "./styles/app-shell.css";
import "./styles/editor-workspace.css";
import "./styles/results-panels.css";

installCspDiagnostics();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
