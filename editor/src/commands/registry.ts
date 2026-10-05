// The command registry: every action the editor offers, with its title, category, icon and key
// bindings. Menus, the command palette, context menus and the keyboard all run commands from here,
// so a shortcut shown in a menu is the one that works.

import type { LucideIcon } from "lucide-react";
import { eventKey, isEditable, normalize } from "./keys";

export type Category = "File" | "Edit" | "Create" | "View" | "Viewport" | "Game" | "Debug" | "Help" | "Panel";

export interface Command {
  id: string;
  title: string;
  category: Category;
  icon?: LucideIcon;
  keys?: string[];
  run(): void | Promise<void>;
  enabled?(): boolean;
  checked?(): boolean;
  /** Also fires while a text field or the code editor has focus. */
  global?: boolean;
  /** Left out of the palette (still bound to keys). */
  hidden?: boolean;
  /** Extra words the palette matches. */
  keywords?: string;
}

const commands = new Map<string, Command>();
const byKey = new Map<string, Command[]>();
const listeners = new Set<() => void>();

export function registerCommands(list: Command[]) {
  for (const c of list) {
    const cmd = { ...c, keys: c.keys?.map(normalize) };
    commands.set(cmd.id, cmd);
    for (const k of cmd.keys ?? []) {
      const arr = byKey.get(k) ?? [];
      byKey.set(k, [...arr.filter((x) => x.id !== cmd.id), cmd]);
    }
  }
  listeners.forEach((l) => l());
}

export function onCommandsChange(fn: () => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

export function getCommand(id: string): Command | undefined {
  return commands.get(id);
}

export function allCommands(): Command[] {
  return [...commands.values()];
}

export function isEnabled(c: Command): boolean {
  try {
    return c.enabled ? c.enabled() : true;
  } catch {
    return false;
  }
}

export function runCommand(id: string) {
  const c = commands.get(id);
  if (!c || !isEnabled(c)) return;
  void c.run();
}

/** Installs the one keyboard listener (capture phase, so it sees keys before Monaco does). */
export function installKeyboard(): () => void {
  const onKey = (e: KeyboardEvent) => {
    if (e.isComposing) return;
    // Held keys repeat only stepping (F10, F11), never toggles such as Play.
    if (e.repeat && e.key !== "F10" && e.key !== "F11") return;
    const list = byKey.get(eventKey(e));
    if (!list) return;
    const target = e.target as HTMLElement | null;
    // Text fields, the code editor and open menus or dialogs handle their own keys; only global
    // commands (the palette, Play, stepping) reach past them.
    const editable = isEditable(target) || !!target?.closest?.(".monaco-editor, .menu, .dialog");
    const cmd = list.find((c) => (!editable || c.global) && isEnabled(c));
    if (!cmd) return;
    e.preventDefault();
    e.stopPropagation();
    void cmd.run();
  };
  window.addEventListener("keydown", onKey, { capture: true });
  return () => window.removeEventListener("keydown", onKey, { capture: true });
}
