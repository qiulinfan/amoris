// The editor's commands. Menus, the palette and the keyboard all come from this list.

import {
  ArrowDownToLine,
  Camera,
  Circle,
  CircleDot,
  Cylinder,
  Flashlight,
  Lightbulb,
  Square,
  Sun,
  ArrowRightToLine,
  ArrowUpFromLine,
  Box,
  Bug,
  CircleStop,
  ClipboardCopy,
  Command as CommandIcon,
  Copy,
  Focus,
  Gauge,
  Globe,
  Grid3x3,
  Hammer,
  Import,
  Info,
  Keyboard,
  LayoutGrid,
  Maximize2,
  Magnet,
  MousePointer2,
  Move3d,
  Pause,
  Pencil,
  Play,
  PlugZap,
  Redo2,
  Rotate3d,
  Save,
  Scale3d,
  StepForward,
  Trash,
  Undo2,
  CirclePlay,
  Unplug,
} from "lucide-react";
import { api, host } from "../host/api";
import { clearAllBreakpoints, pauseScripts, resume, step } from "../actions/debug";
import { applyScripts, saveActiveScript, toggleBreakpointAtCursor } from "../actions/scripts";
import { play, saveProject, setSpeed, stepTick, stop, togglePause, togglePlay } from "../actions/time";
import { deleteEntities, duplicateEntities, PRIMITIVES, primitiveAvailable, redo, spawnPrimitive, undo } from "../actions/world";
import { importAsset } from "../panels/assets/AssetsPanel";
import { openPanel, resetLayout, togglePanel, isPanelOpen, toggleMaximize } from "../layout/dock";
import { PANELS } from "../layout/panels";
import { useConnection } from "../state/connection";
import { useDebug } from "../state/debug";
import { useHistory } from "../state/history";
import { useSelection } from "../state/selection";
import { useSession } from "../state/session";
import { promptText, useUi } from "../state/ui";
import { dragState, useViewport } from "../state/viewport";
import { useWorld } from "../state/world";
import { registerCommands, type Command } from "./registry";

const PRIMITIVE_ICONS = { empty: CircleDot, cube: Box, sphere: Circle, cylinder: Cylinder, plane: Square, directional: Sun, point: Lightbulb, spot: Flashlight, camera: Camera };

const sel = () => useSelection.getState().ids;
const hasSel = () => sel().length > 0;
const connected = () => useConnection.getState().info.state === "open";
const mode = () => useSession.getState().status?.mode;
const paused = () => useDebug.getState().state.state === "paused";
const inScripts = () => !!document.activeElement?.closest(".panel-scripts");

export function registerBuiltinCommands() {
  const commands: Command[] = [
    // ---- File
    {
      id: "file.save",
      title: "Save",
      category: "File",
      icon: Save,
      keys: ["Mod+S"],
      global: true,
      keywords: "save scene script",
      run: () => {
        if (inScripts() && saveActiveScript()) return;
        void saveProject().then((r) => r && useUi.getState().toast({ kind: "success", title: "Saved", body: r.files.join(", ") }));
      },
    },
    { id: "file.saveScene", title: "Save Scene", category: "File", icon: Save, enabled: connected, run: () => void saveProject().then((r) => r && useUi.getState().toast({ kind: "success", title: "Scene saved", body: r.files.join(", ") })) },
    { id: "file.import", title: "Import Asset…", category: "File", icon: Import, enabled: connected, run: () => void importAsset() },
    { id: "file.reconnect", title: "Reconnect to Host", category: "File", icon: PlugZap, run: () => host.retryNow() },
    {
      id: "file.connect",
      title: "Connect to Host…",
      category: "File",
      icon: Unplug,
      keywords: "host url port mock",
      run: async () => {
        const v = await promptText({ title: "Connect to a host", label: "Host address (empty: the one serving the editor)", value: new URLSearchParams(location.search).get("host") ?? "", placeholder: "127.0.0.1:7878", confirm: "Connect" });
        if (v === null) return;
        const u = new URL(location.href);
        if (v.trim()) u.searchParams.set("host", v.trim());
        else u.searchParams.delete("host");
        location.href = u.toString();
      },
    },
    // ---- Edit
    { id: "edit.undo", title: "Undo", category: "Edit", icon: Undo2, keys: ["Mod+Z"], enabled: () => useHistory.getState().undo.length > 0, run: () => void undo() },
    { id: "edit.redo", title: "Redo", category: "Edit", icon: Redo2, keys: ["Mod+Shift+Z", "Mod+Y"], enabled: () => useHistory.getState().redo.length > 0, run: () => void redo() },
    { id: "edit.duplicate", title: "Duplicate", category: "Edit", icon: Copy, keys: ["Mod+D"], enabled: hasSel, run: () => void duplicateEntities(sel()) },
    { id: "edit.delete", title: "Delete", category: "Edit", icon: Trash, keys: ["Delete", "Backspace"], enabled: hasSel, run: () => void deleteEntities(sel()) },
    {
      id: "edit.rename",
      title: "Rename",
      category: "Edit",
      icon: Pencil,
      keys: ["F2"],
      enabled: () => useSelection.getState().primary !== null,
      run: () => {
        openPanel("hierarchy", false);
        useUi.getState().set({ renaming: useSelection.getState().primary });
      },
    },
    { id: "edit.selectAll", title: "Select All", category: "Edit", keys: ["Mod+A"], run: () => useSelection.getState().set([...useWorld.getState().order]) },
    { id: "edit.deselect", title: "Deselect", category: "Edit", keys: ["Escape"], enabled: () => hasSel() && !dragState.active, run: () => useSelection.getState().clear() },
    {
      id: "edit.copyIds",
      title: "Copy Entity IDs",
      category: "Edit",
      icon: ClipboardCopy,
      enabled: hasSel,
      run: () => void navigator.clipboard?.writeText(sel().join(", ")),
    },
    { id: "edit.palette", title: "Command Palette", category: "Edit", icon: CommandIcon, keys: ["Mod+K", "Mod+Shift+P"], global: true, run: () => useUi.getState().set({ paletteOpen: !useUi.getState().paletteOpen, paletteQuery: "" }) },
    // ---- Create
    ...PRIMITIVES.map((p) => ({
      id: `create.${p.kind}`,
      title: `Create ${p.label}`,
      category: "Create" as const,
      icon: PRIMITIVE_ICONS[p.kind],
      keywords: "new spawn entity add",
      enabled: () => connected() && primitiveAvailable(p.kind),
      run: () => void spawnPrimitive(p.kind),
    })),
    // ---- View
    ...PANELS.map((p, i) => ({
      id: `view.panel.${p.id}`,
      title: `${p.title}`,
      category: "Panel" as const,
      icon: p.icon,
      keys: i < 9 ? [`Mod+Alt+${i + 1}`] : undefined,
      keywords: "panel show open window",
      checked: () => isPanelOpen(p.id),
      run: () => openPanel(p.id),
    })),
    ...PANELS.map((p) => ({
      id: `view.toggle.${p.id}`,
      title: `Toggle ${p.title} Panel`,
      category: "View" as const,
      icon: p.icon,
      hidden: true,
      run: () => togglePanel(p.id),
    })),
    { id: "view.resetLayout", title: "Reset Layout", category: "View", icon: LayoutGrid, run: () => resetLayout() },
    { id: "view.maximize", title: "Maximize Panel / Restore", category: "View", icon: Maximize2, keys: ["Shift+Space"], run: () => toggleMaximize() },
    { id: "view.grid", title: "Toggle Grid", category: "Viewport", icon: Grid3x3, keys: ["G"], checked: () => useViewport.getState().grid, run: () => useViewport.getState().set({ grid: !useViewport.getState().grid }) },
    { id: "view.stats", title: "Toggle Viewport Stats", category: "Viewport", icon: Gauge, checked: () => useViewport.getState().stats, run: () => useViewport.getState().set({ stats: !useViewport.getState().stats }) },
    { id: "view.frame", title: "Frame Selection", category: "Viewport", icon: Focus, keys: ["F"], run: () => useViewport.getState().frame() },
    { id: "view.tool.select", title: "Select Tool", category: "Viewport", icon: MousePointer2, keys: ["Q"], checked: () => useViewport.getState().tool === "select", run: () => useViewport.getState().set({ tool: "select" }) },
    { id: "view.tool.translate", title: "Move Tool", category: "Viewport", icon: Move3d, keys: ["W"], checked: () => useViewport.getState().tool === "translate", run: () => useViewport.getState().set({ tool: "translate" }) },
    { id: "view.tool.rotate", title: "Rotate Tool", category: "Viewport", icon: Rotate3d, keys: ["E"], checked: () => useViewport.getState().tool === "rotate", run: () => useViewport.getState().set({ tool: "rotate" }) },
    { id: "view.tool.scale", title: "Scale Tool", category: "Viewport", icon: Scale3d, keys: ["R"], checked: () => useViewport.getState().tool === "scale", run: () => useViewport.getState().set({ tool: "scale" }) },
    {
      id: "view.space",
      title: "Toggle Local/World Space",
      category: "Viewport",
      icon: Globe,
      keys: ["X"],
      run: () => useViewport.getState().set({ space: useViewport.getState().space === "world" ? "local" : "world" }),
    },
    { id: "view.snap", title: "Toggle Snapping", category: "Viewport", icon: Magnet, checked: () => useViewport.getState().snap.enabled, run: () => useViewport.getState().setSnap({ enabled: !useViewport.getState().snap.enabled }) },
    { id: "view.top", title: "View From Top", category: "Viewport", keys: ["Alt+7"], run: () => useViewport.getState().look("+y") },
    { id: "view.front", title: "View From Front", category: "Viewport", keys: ["Alt+1"], run: () => useViewport.getState().look("+z") },
    { id: "view.right", title: "View From Right", category: "Viewport", keys: ["Alt+3"], run: () => useViewport.getState().look("+x") },
    { id: "view.ortho", title: "Toggle Perspective/Orthographic", category: "Viewport", keys: ["Alt+5"], run: () => useViewport.getState().look("persp") },
    // ---- Game
    { id: "game.play", title: "Play / Stop", category: "Game", icon: Play, keys: ["Mod+P"], global: true, enabled: connected, run: () => void togglePlay() },
    { id: "game.pause", title: "Pause / Resume", category: "Game", icon: Pause, keys: ["Mod+Alt+P"], global: true, enabled: connected, run: () => void togglePause() },
    { id: "game.step", title: "Step One Tick", category: "Game", icon: StepForward, keys: ["Mod+Alt+."], global: true, enabled: connected, run: () => void stepTick() },
    { id: "game.stop", title: "Stop", category: "Game", icon: CircleStop, keys: ["Shift+F5"], global: true, enabled: () => mode() === "play", run: () => void stop() },
    ...[0.25, 0.5, 1, 2, 4].map((s) => ({
      id: `game.speed.${s}`,
      title: `Speed ${s}×`,
      category: "Game" as const,
      icon: Gauge,
      checked: () => (useSession.getState().status?.speed ?? 1) === s,
      enabled: connected,
      run: () => void setSpeed(s),
    })),
    { id: "game.apply", title: "Apply Scripts", category: "Game", icon: Hammer, keys: ["Mod+Shift+B"], global: true, enabled: connected, run: () => void applyScripts() },
    // ---- Debug
    {
      id: "debug.continue",
      title: "Continue / Start",
      category: "Debug",
      icon: CirclePlay,
      keys: ["F5"],
      global: true,
      enabled: connected,
      run: () => {
        if (paused()) void resume();
        else if (mode() !== "play") void play();
      },
    },
    { id: "debug.pause", title: "Pause Scripts", category: "Debug", icon: Pause, keys: ["F6"], global: true, enabled: () => mode() === "play" && !paused(), run: () => void pauseScripts() },
    { id: "debug.stepOver", title: "Step Over", category: "Debug", icon: ArrowRightToLine, keys: ["F10"], global: true, enabled: paused, run: () => void step("over") },
    { id: "debug.stepInto", title: "Step Into", category: "Debug", icon: ArrowDownToLine, keys: ["F11"], global: true, enabled: paused, run: () => void step("into") },
    { id: "debug.stepOut", title: "Step Out", category: "Debug", icon: ArrowUpFromLine, keys: ["Shift+F11"], global: true, enabled: paused, run: () => void step("out") },
    { id: "debug.toggleBreakpoint", title: "Toggle Breakpoint", category: "Debug", icon: Bug, keys: ["F9"], global: true, run: () => toggleBreakpointAtCursor() },
    { id: "debug.clearBreakpoints", title: "Remove All Breakpoints", category: "Debug", icon: Trash, enabled: () => useDebug.getState().breakpoints.length > 0, run: () => void clearAllBreakpoints() },
    {
      id: "debug.devtools",
      title: "Copy Chrome DevTools URL",
      category: "Debug",
      icon: ClipboardCopy,
      keywords: "cdp inspector vscode attach",
      run: async () => {
        // The host's CDP endpoint is pocket-debug's own port (9229 by default), not the host's.
        const cdp = (await api.debug.state().catch(() => null))?.cdp ?? useDebug.getState().cdp;
        if (!cdp) {
          useUi.getState().toast({ kind: "error", title: "No CDP endpoint", body: "This host does not serve the Chrome DevTools Protocol (its port was taken, or it is the mock)." });
          return;
        }
        void navigator.clipboard?.writeText(cdp.devtools);
        useUi.getState().toast({ kind: "info", title: "DevTools URL copied", body: `${cdp.devtools} — paste it in Chrome's address bar (VS Code attaches to ${cdp.ws}).` });
      },
    },
    {
      id: "debug.state",
      title: "Refresh Debugger State",
      category: "Debug",
      hidden: false,
      enabled: connected,
      run: () => void api.debug.state().then((s) => useDebug.getState().setState(s)),
    },
    // ---- Help
    { id: "help.shortcuts", title: "Keyboard Shortcuts", category: "Help", icon: Keyboard, keys: ["Mod+/"], global: true, run: () => useUi.getState().set({ shortcutsOpen: true }) },
    { id: "help.catalog", title: "Browse Host Commands…", category: "Help", icon: CommandIcon, keywords: "catalog methods api protocol", run: () => useUi.getState().set({ paletteOpen: true, paletteQuery: "call " }) },
    { id: "help.about", title: "About the Editor", category: "Help", icon: Info, run: () => useUi.getState().set({ aboutOpen: true }) },
  ];
  registerCommands(commands);
}
