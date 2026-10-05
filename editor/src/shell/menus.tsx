// The menu bar's menus, as lists of command ids (and submenus); entries are built from the commands
// when a menu opens, so enabled and checked states are current.

import { PANELS } from "../layout/panels";
import { PRIMITIVES } from "../actions/world";
import { getCommand, isEnabled } from "../commands/registry";
import type { MenuEntry } from "../state/ui";

type Spec = string | "-" | { sub: string; items: Spec[]; strip?: string };

export const MENUS: { label: string; items: Spec[] }[] = [
  { label: "File", items: ["file.save", "file.saveScene", "-", "file.import", "-", "file.connect", "file.reconnect"] },
  {
    label: "Edit",
    items: [
      "edit.undo",
      "edit.redo",
      "-",
      "edit.duplicate",
      "edit.delete",
      "edit.rename",
      "-",
      "edit.selectAll",
      "edit.deselect",
      "edit.copyIds",
      "-",
      { sub: "Create", items: PRIMITIVES.flatMap((p, i) => (i === 1 || i === 5 || i === 8 ? ["-", `create.${p.kind}`] : [`create.${p.kind}`])), strip: "Create " },
      "-",
      "edit.palette",
    ],
  },
  {
    label: "View",
    items: [
      { sub: "Panels", items: PANELS.map((p) => `view.panel.${p.id}`) },
      "view.maximize",
      "view.resetLayout",
      "-",
      "view.tool.select",
      "view.tool.translate",
      "view.tool.rotate",
      "view.tool.scale",
      "view.space",
      "view.snap",
      "-",
      "view.grid",
      "view.stats",
      "view.frame",
      { sub: "Camera", items: ["view.top", "view.front", "view.right", "view.ortho"] },
    ],
  },
  {
    label: "Game",
    items: ["game.play", "game.pause", "game.step", "game.stop", "-", { sub: "Speed", items: ["game.speed.0.25", "game.speed.0.5", "game.speed.1", "game.speed.2", "game.speed.4"] }, "-", "game.apply"],
  },
  {
    label: "Debug",
    items: ["debug.continue", "debug.pause", "debug.stepOver", "debug.stepInto", "debug.stepOut", "-", "debug.toggleBreakpoint", "debug.clearBreakpoints", "-", "debug.devtools", "view.panel.debug"],
  },
  { label: "Help", items: ["help.shortcuts", "help.catalog", "-", "help.about"] },
];

export function buildMenu(items: Spec[], strip = ""): MenuEntry[] {
  const out: MenuEntry[] = [];
  for (const s of items) {
    if (s === "-") {
      out.push("separator");
      continue;
    }
    if (typeof s === "object") {
      out.push({ label: s.sub, submenu: buildMenu(s.items, s.strip) });
      continue;
    }
    const c = getCommand(s);
    if (!c) continue;
    const Icon = c.icon;
    out.push({
      id: c.id,
      label: strip && c.title.startsWith(strip) ? c.title.slice(strip.length) : c.title,
      icon: Icon ? <Icon size={14} /> : undefined,
      keys: c.keys?.[0],
      disabled: !isEnabled(c),
      checked: c.checked?.(),
      run: () => void c.run(),
    });
  }
  return out;
}
