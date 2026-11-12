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
import { Boundary } from "./ui/Boundary";
import { ContextMenuHost } from "./ui/Menu";
import { Toasts } from "./ui/Toasts";
import { TooltipHost } from "./ui/Tooltip";
import { cx } from "./ui/cx";

export function App() {
  const mode = useSession((s) => s.status?.mode);
  const paused = useDebug((s) => s.state.state === "paused");
  useEffect(() => {
    document.title = `${mode === "play" ? "▶ " : ""}Amoris Editor`;
  }, [mode]);
  return (
    <div className={cx("app", mode === "play" && "mode-play", paused && "is-debug-paused")}>
      {/* Each part fails alone: a status a bar cannot draw leaves the panels and the transport up
          (every dock panel has a boundary of its own, layout/DockLayout.tsx). */}
      <Boundary name="The top bar">
        <TopBar />
      </Boundary>
      <main className="app-main">
        <DockLayout />
      </main>
      <Boundary name="The status bar">
        <StatusBar />
      </Boundary>
      <Boundary name="An overlay">
        <ContextMenuHost />
        <CommandPalette />
        <PromptDialog />
        <ShortcutsDialog />
        <AboutDialog />
        <Toasts />
        <TooltipHost />
      </Boundary>
    </div>
  );
}
