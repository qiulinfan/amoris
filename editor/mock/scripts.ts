// The mock's scripts: the sailing sample's TypeScript files, read once from samples/sailing/scripts
// and then kept in memory (writes never touch the sample), with fake but plausible diagnostics:
// bracket balance, the stateless-script rules of charter 3.3 and unknown component names.

import { readdirSync, readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { COMPONENTS } from "./schemas";
import { hashJson, suggest } from "./util";

export interface Diagnostic {
  file: string;
  line: number;
  column: number;
  end_line: number;
  end_column: number;
  severity: "error" | "warning" | "info" | "hint";
  message: string;
  code: string;
}

const SAMPLE = resolve(import.meta.dir, "../../samples/sailing/scripts");

export class Scripts {
  files = new Map<string, string>();

  constructor() {
    try {
      for (const f of readdirSync(SAMPLE).sort()) {
        if (f.endsWith(".ts")) this.files.set(`scripts/${f}`, readFileSync(join(SAMPLE, f), "utf8"));
      }
    } catch {
      this.files.set("scripts/main.ts", 'import { game } from "pocket";\n\nexport default game({ components: [], systems: [] });\n');
    }
  }

  list() {
    return [...this.files.entries()].map(([path, text]) => ({
      path,
      bytes: new TextEncoder().encode(text).length,
      diagnostics: diagnose(path, text),
    }));
  }

  bundleHash(): string {
    return hashJson([...this.files.entries()]).slice(0, 12);
  }
}

const COMPONENT_NAMES = [...COMPONENTS.map((c) => c.name), "Name"];
const PAIRS: Record<string, string> = { ")": "(", "]": "[", "}": "{" };

export function diagnose(file: string, text: string): Diagnostic[] {
  const out: Diagnostic[] = [];
  const lines = text.split("\n");
  const at = (line: number, column: number, length: number, severity: Diagnostic["severity"], code: string, message: string) =>
    out.push({ file, line, column, end_line: line, end_column: column + Math.max(1, length), severity, code, message });

  // Brackets, outside strings and comments.
  const stack: { ch: string; line: number; column: number }[] = [];
  let inString: string | null = null;
  let inBlock = false;
  lines.forEach((src, li) => {
    let inLineComment = false;
    for (let ci = 0; ci < src.length; ci++) {
      const ch = src[ci]!;
      const next = src[ci + 1];
      if (inLineComment) break;
      if (inBlock) {
        if (ch === "*" && next === "/") {
          inBlock = false;
          ci++;
        }
        continue;
      }
      if (inString) {
        if (ch === "\\") ci++;
        else if (ch === inString) inString = null;
        continue;
      }
      if (ch === "/" && next === "/") inLineComment = true;
      else if (ch === "/" && next === "*") inBlock = true;
      else if (ch === '"' || ch === "'" || ch === "`") inString = ch;
      else if (ch === "(" || ch === "[" || ch === "{") stack.push({ ch, line: li + 1, column: ci + 1 });
      else if (ch === ")" || ch === "]" || ch === "}") {
        const open = stack.pop();
        if (!open || open.ch !== PAIRS[ch]) {
          at(li + 1, ci + 1, 1, "error", "TS1128", `Unexpected '${ch}'.`);
          return;
        }
      }
    }
    if (inString === '"' || inString === "'") inString = null;
  });
  for (const open of stack.slice(-1)) {
    const close = open.ch === "(" ? ")" : open.ch === "[" ? "]" : "}";
    at(open.line, open.column, 1, "error", "TS1005", `'${close}' expected to close this '${open.ch}'.`);
  }

  lines.forEach((src, li) => {
    const line = li + 1;
    const top = /^(let|var)\s+(\w+)/.exec(src);
    if (top) {
      at(line, 1, top[0].length, "error", "script.module_state",
        `Module-level '${top[1]} ${top[2]}' keeps game state outside the world; scripts are stateless systems (charter 3.3). Put it in a component.`);
    }
    for (const m of src.matchAll(/Date\.now\(\)|performance\.now\(\)/g)) {
      at(line, (m.index ?? 0) + 1, m[0].length, "error", "script.clock", "Scripts read no clock; use ctx.time or ctx.tick.");
    }
    for (const m of src.matchAll(/Math\.random\(\)/g)) {
      at(line, (m.index ?? 0) + 1, m[0].length, "info", "script.random",
        "Math.random() draws from the system stream; ctx.rng names it explicitly.");
    }
    for (const m of src.matchAll(/:\s*any\b/g)) {
      at(line, (m.index ?? 0) + 1, m[0].length, "hint", "script.any", "'any' hides the generated component types.");
    }
    // Component names in query specs and world calls.
    for (const m of src.matchAll(/(?:with:\s*\[|ctx\.world\.(?:get|set|has|insert|remove)\([^,]+,\s*|ctx\.single\()([^\]\)]*)/g)) {
      const start = (m.index ?? 0) + m[0].length - m[1]!.length;
      for (const s of m[1]!.matchAll(/"([A-Za-z0-9_.]+)"/g)) {
        const name = s[1]!.split(".")[0]!;
        if (!COMPONENT_NAMES.includes(name)) {
          const did = suggest(name, COMPONENT_NAMES);
          at(line, start + (s.index ?? 0) + 2, name.length, "error", "script.unknown_component",
            `There is no component '${name}'${did.length ? `; did you mean '${did[0]}'?` : "."}`);
        }
      }
    }
  });
  return out;
}

/** The line ranges of each `system({...})` in a file and of its `run` body. */
export function systemRanges(text: string): { name: string; start: number; end: number; body: number }[] {
  const lines = text.split("\n");
  const out: { name: string; start: number; end: number; body: number }[] = [];
  lines.forEach((src, i) => {
    const m = /export const (\w+) = system\(\{/.exec(src);
    if (!m) return;
    let end = i;
    for (let j = i + 1; j < lines.length; j++) {
      if (/^\}\);/.test(lines[j]!)) {
        end = j;
        break;
      }
    }
    let body = i;
    for (let j = i; j <= end; j++) {
      if (/\brun\(/.test(lines[j]!)) {
        body = j + 1;
        break;
      }
    }
    const nameLine = lines.slice(i, end).join("\n");
    const name = /name:\s*"([a-z_0-9]+)"/.exec(nameLine)?.[1] ?? m[1]!;
    out.push({ name, start: i + 1, end: end + 1, body: body + 1 });
  });
  return out;
}
