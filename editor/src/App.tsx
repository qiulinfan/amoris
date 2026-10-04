// The editor window: top bar (menus, transport, palette), the dock, the status bar, and the
// overlays every panel shares (context menu, palette, dialogs, toasts, tooltip).

import { useEffect } from "react";
import { DockLayout } from "./layout/DockLayout";
import { useSession } from "./state/session";
import { useDebug } from "./state/debug";
import { TopBar } from "./shell/TopBar";
import { StatusBar } from "./shell/StatusBar";
import { CommandPalette } from "./shell/CommandPalette";
import { AboutDialog, PromptDialog, ShortcutsDialog } from "./shell/Dialogs";
import { ContextMenuHost } from "./ui/Menu";
import { Toasts } from "./ui/Toasts";
import { TooltipHost } from "./ui/Tooltip";
import { cx } from "./ui/cx";

export function App() {
  const mode = useSession((s) => s.status?.mode);
  const paused = useDebug((s) => s.state.state === "paused");
  useEffect(() => {
    document.title = `${mode === "play" ? "▶ " : ""}Pocket3D Editor`;
  }, [mode]);
  return (
    <div className={cx("app", mode === "play" && "mode-play", paused && "is-debug-paused")}>
      <TopBar />
      <main className="app-main">
        <DockLayout />
      </main>
      <StatusBar />
      <ContextMenuHost />
      <CommandPalette />
      <PromptDialog />
      <ShortcutsDialog />
      <AboutDialog />
      <Toasts />
      <TooltipHost />
    </div>
  );
}
