import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "@fontsource-variable/inter";
import "@fontsource-variable/jetbrains-mono";
import "dockview-react/dist/styles/dockview.css";
import "./styles/theme.css";
import "./styles/ui.css";
import "./styles/shell.css";
import "./styles/dock.css";
import { App } from "./App";
import { registerBuiltinCommands } from "./commands/builtin";
import { installKeyboard } from "./commands/registry";
import { startSync } from "./host/sync";
import { installDesktop } from "./desktop";

registerBuiltinCommands();
installKeyboard();
installDesktop();
startSync();

// The editor is driven by people and by tests: expose the stores for inspection in DevTools.
if (import.meta.env.DEV) {
  void import("./devtools").then((m) => m.expose());
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
