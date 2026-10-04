// Key combinations: one normal form for bindings and events ("Mod+Shift+Z", Mod being Cmd on macOS
// and Ctrl elsewhere), and how to show them.

export const isMac = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);

const MOD_ORDER = ["Mod", "Ctrl", "Alt", "Shift"];

const CODE_KEYS: Record<string, string> = {
  Period: ".",
  Comma: ",",
  Slash: "/",
  Backslash: "\\",
  Backquote: "`",
  BracketLeft: "[",
  BracketRight: "]",
  Semicolon: ";",
  Quote: "'",
  Minus: "-",
  Equal: "=",
  Space: "Space",
};

/** Puts a binding into normal form (modifier order, key case). */
export function normalize(binding: string): string {
  const parts = binding.split("+").map((p) => p.trim()).filter(Boolean);
  const key = parts.pop() ?? "";
  const mods = MOD_ORDER.filter((m) => parts.some((p) => p.toLowerCase() === m.toLowerCase()));
  return [...mods, key.length === 1 ? key.toUpperCase() : key].join("+");
}

export function eventKey(e: KeyboardEvent): string {
  let key: string;
  if (/^Key[A-Z]$/.test(e.code)) key = e.code.slice(3);
  else if (/^Digit\d$/.test(e.code)) key = e.code.slice(5);
  else if (CODE_KEYS[e.code]) key = CODE_KEYS[e.code]!;
  else key = e.key.length === 1 ? e.key.toUpperCase() : e.key;
  const mods: string[] = [];
  if (isMac ? e.metaKey : e.ctrlKey) mods.push("Mod");
  if (isMac && e.ctrlKey) mods.push("Ctrl");
  if (e.altKey) mods.push("Alt");
  if (e.shiftKey) mods.push("Shift");
  return [...mods, key].join("+");
}

const MAC_SYMBOLS: Record<string, string> = { Mod: "⌘", Ctrl: "⌃", Alt: "⌥", Shift: "⇧" };
const KEY_NAMES: Record<string, string> = {
  ArrowUp: "↑",
  ArrowDown: "↓",
  ArrowLeft: "←",
  ArrowRight: "→",
  Escape: "Esc",
  Delete: isMac ? "⌦" : "Del",
  Backspace: isMac ? "⌫" : "Backspace",
  Enter: "↵",
  Space: "Space",
};

/** The parts of a binding as shown on key caps. */
export function keyCaps(binding: string): string[] {
  const parts = normalize(binding).split("+");
  const key = parts.pop()!;
  const mods = parts.map((m) => (isMac ? MAC_SYMBOLS[m]! : m === "Mod" ? "Ctrl" : m));
  return [...mods, KEY_NAMES[key] ?? key];
}

export function formatKeys(binding: string): string {
  return keyCaps(binding).join(isMac ? "" : "+");
}

export function isEditable(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null;
  if (!el || !el.tagName) return false;
  if (el.isContentEditable) return true;
  if (el.tagName === "TEXTAREA" || el.tagName === "SELECT") return true;
  if (el.tagName === "INPUT") {
    const type = (el as HTMLInputElement).type;
    return !["checkbox", "radio", "button", "range", "color"].includes(type);
  }
  return false;
}
