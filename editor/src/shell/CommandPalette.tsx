// The command palette (Cmd/Ctrl+K, Ctrl+Shift+P): every editor command, every panel, every entity
// and every method of the host's catalog, fuzzy-matched. Prefixes narrow it: ">" commands, "@"
// entities, "call " host methods. A host method opens a params editor (a skeleton from its params
// schema) and shows the result, which is how a person calls exactly what an agent can.

import { Braces, CornerDownLeft, Play, Terminal } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { host } from "../host/api";
import { HostError } from "../host/client";
import type { CatalogCommand } from "../host/protocol";
import { allCommands, isEnabled, type Command } from "../commands/registry";
import { useConnection } from "../state/connection";
import { logLocal } from "../state/logs";
import { useSelection } from "../state/selection";
import { useUi } from "../state/ui";
import { useViewport } from "../state/viewport";
import { useWorld } from "../state/world";
import { Dialog } from "../ui/Dialog";
import { Kbd } from "../ui/Kbd";
import { entityKind } from "../ui/icons";
import { cx } from "../ui/cx";

interface Item {
  key: string;
  section: string;
  title: string;
  detail?: string;
  icon?: React.ReactNode;
  keys?: string;
  disabled?: boolean;
  run(): void;
  score: number;
  match?: number[];
}

/** Subsequence match with bonuses for word starts and runs; null when it does not match. */
function fuzzy(query: string, text: string): { score: number; idx: number[] } | null {
  if (!query) return { score: 0, idx: [] };
  const q = query.toLowerCase();
  const t = text.toLowerCase();
  const idx: number[] = [];
  let score = 0;
  let ti = 0;
  let prev = -2;
  for (const ch of q) {
    if (ch === " ") continue;
    const at = t.indexOf(ch, ti);
    if (at < 0) return null;
    const wordStart = at === 0 || /[\s._\-/:]/.test(t[at - 1]!) || (text[at] !== t[at] && text[at - 1] === t[at - 1]);
    score += 1 + (wordStart ? 6 : 0) + (at === prev + 1 ? 4 : 0) - Math.min(3, (at - ti) * 0.05);
    idx.push(at);
    prev = at;
    ti = at + 1;
  }
  if (t.startsWith(q)) score += 10;
  return { score, idx };
}

function Highlight({ text, idx }: { text: string; idx?: number[] }) {
  if (!idx?.length) return <>{text}</>;
  const set = new Set(idx);
  return (
    <>
      {[...text].map((c, i) =>
        set.has(i) ? (
          <mark key={i}>{c}</mark>
        ) : (
          <span key={i}>{c}</span>
        ),
      )}
    </>
  );
}

function skeleton(cmd: CatalogCommand): string {
  const schema = cmd.params as { properties?: Record<string, { type?: string | string[] }>; required?: string[] } | undefined;
  const out: Record<string, unknown> = {};
  for (const k of schema?.required ?? []) {
    const t = schema?.properties?.[k]?.type;
    const ty = Array.isArray(t) ? t[0] : t;
    out[k] = ty === "integer" || ty === "number" ? 0 : ty === "array" ? [] : ty === "object" ? {} : ty === "boolean" ? false : "";
  }
  if (cmd.name === "world.get" || cmd.name === "debug.watch") {
    const sel = useSelection.getState().primary;
    if (sel !== null) out.entity = sel;
  }
  return JSON.stringify(out);
}

function MethodRunner({ cmd, onBack }: { cmd: CatalogCommand; onBack(): void }) {
  const [params, setParams] = useState(() => skeleton(cmd));
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);
  const [busy, setBusy] = useState(false);
  const props = (cmd.params as { properties?: Record<string, { type?: unknown; description?: string }>; required?: string[] } | undefined) ?? {};
  const run = async () => {
    let p: unknown;
    try {
      p = params.trim() ? JSON.parse(params) : {};
    } catch (e) {
      setResult({ ok: false, text: `Params are not JSON: ${e instanceof Error ? e.message : String(e)}` });
      return;
    }
    setBusy(true);
    try {
      const r = await host.callRaw(cmd.name, p);
      const text = JSON.stringify(r, null, 2);
      setResult({ ok: true, text: text.length > 20000 ? `${text.slice(0, 20000)}\n…` : text });
      logLocal("result", `${cmd.name} ${params} → ${JSON.stringify(r).slice(0, 300)}`, { source: "palette" });
    } catch (e) {
      const msg = e instanceof HostError ? `${e.code}: ${e.message}${e.suggestions.length ? `\ndid you mean: ${e.suggestions.join(", ")}` : ""}` : String(e);
      setResult({ ok: false, text: msg });
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="runner">
      <div className="runner-head">
        <button type="button" className="link" onClick={onBack}>
          ← all results
        </button>
        <span className="runner-name mono">{cmd.name}</span>
        <span className={`kind-tag kind-${cmd.kind}`}>{cmd.kind}</span>
      </div>
      <div className="runner-doc">{cmd.doc}</div>
      {props.properties && Object.keys(props.properties).length > 0 && (
        <div className="runner-params">
          {Object.entries(props.properties).map(([k, v]) => (
            <div key={k} className="runner-param">
              <span className="mono">{k}</span>
              {props.required?.includes(k) && <span className="req">required</span>}
              <span className="muted mono">{Array.isArray(v.type) ? v.type.join("|") : String(v.type ?? "any")}</span>
              {v.description && <span className="muted">{v.description}</span>}
            </div>
          ))}
        </div>
      )}
      <div className="runner-input">
        <Braces size={14} />
        <input
          autoFocus
          className="mono"
          value={params}
          spellCheck={false}
          onChange={(e) => setParams(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              void run();
            }
            if (e.key === "Escape" || (e.key === "Backspace" && !params)) {
              e.stopPropagation();
              e.preventDefault();
              onBack();
            }
          }}
        />
        <button type="button" className="btn btn-primary btn-sm" disabled={busy} onClick={() => void run()}>
          <Play size={12} /> <span>Call</span>
        </button>
      </div>
      {result && <pre className={cx("runner-result", !result.ok && "is-error")}>{result.text}</pre>}
    </div>
  );
}

export function CommandPalette() {
  const open = useUi((s) => s.paletteOpen);
  const initial = useUi((s) => s.paletteQuery);
  const close = () => useUi.getState().set({ paletteOpen: false });
  if (!open) return null;
  return <Palette initial={initial} onClose={close} />;
}

function Palette({ initial, onClose }: { initial: string; onClose(): void }) {
  const [q, setQ] = useState(initial);
  const [active, setActive] = useState(0);
  const [method, setMethod] = useState<CatalogCommand | null>(null);
  const catalog = useConnection((s) => s.catalog);
  const nodes = useWorld((s) => s.nodes);
  const order = useWorld((s) => s.order);
  const list = useRef<HTMLDivElement>(null);

  const items = useMemo(() => {
    let query = q.trim();
    let only: "cmd" | "ent" | "call" | null = null;
    if (query.startsWith(">")) {
      only = "cmd";
      query = query.slice(1).trim();
    } else if (query.startsWith("@")) {
      only = "ent";
      query = query.slice(1).trim();
    } else if (/^call\b/i.test(query)) {
      only = "call";
      query = query.slice(4).trim();
    }
    const out: Item[] = [];
    if (!only || only === "cmd") {
      for (const c of allCommands()) {
        if (c.hidden) continue;
        const label = `${c.category === "Panel" ? "Open " : ""}${c.title}`;
        const m = fuzzy(query, label) ?? (c.keywords && query.length >= 3 && c.keywords.includes(query.toLowerCase()) ? { score: 1, idx: [] } : null);
        if (!m) continue;
        const Icon = c.icon;
        out.push({
          key: `c:${c.id}`,
          section: c.category === "Panel" ? "Panels" : "Commands",
          title: label,
          detail: c.category,
          icon: Icon ? <Icon size={14} /> : undefined,
          keys: c.keys?.[0],
          disabled: !isEnabled(c),
          run: () => runCommand(c, onClose),
          score: m.score + (c.category === "Panel" ? -1 : 0),
          match: fuzzy(query, label)?.idx,
        });
      }
    }
    if ((!only && query) || only === "ent") {
      for (const id of order) {
        const n = nodes[id]!;
        const m = fuzzy(query, n.name) ?? (query === `#${id}` || query === String(id) ? { score: 20, idx: [] } : null);
        if (!m) continue;
        const Icon = entityKind(n.components).icon;
        out.push({
          key: `e:${id}`,
          section: "Entities",
          title: n.name,
          detail: `#${id} · ${n.components.slice(0, 4).join(", ")}${n.components.length > 4 ? "…" : ""}`,
          icon: <Icon size={14} />,
          run: () => {
            useSelection.getState().set([id]);
            useViewport.getState().frame();
            onClose();
          },
          score: m.score - 2,
          match: m.idx,
        });
      }
    }
    if ((!only && query) || only === "call") {
      for (const c of catalog) {
        const m = fuzzy(query, c.name) ?? (query.length >= 3 && c.doc.toLowerCase().includes(query.toLowerCase()) ? { score: 0, idx: [] } : null);
        if (!m) continue;
        out.push({
          key: `m:${c.name}`,
          section: "Host methods",
          title: c.name,
          detail: c.doc,
          icon: <Terminal size={14} />,
          run: () => setMethod(c),
          score: m.score - 3,
          match: fuzzy(query, c.name)?.idx,
        });
      }
    }
    const sectionOrder = ["Commands", "Panels", "Entities", "Host methods"];
    if (query) out.sort((a, b) => b.score - a.score);
    else out.sort((a, b) => sectionOrder.indexOf(a.section) - sectionOrder.indexOf(b.section));
    return out.slice(0, 200);
  }, [q, catalog, nodes, order, onClose]);

  useEffect(() => setActive(0), [q]);
  useEffect(() => {
    list.current?.querySelector(".pal-item.is-active")?.scrollIntoView({ block: "nearest" });
  }, [active]);

  // Group consecutive items of a section only when not ranking by score.
  const grouped = !q.trim() || /^[>@]|^call\b/i.test(q.trim());

  return (
    <Dialog onClose={onClose} className="palette" top>
      {method ? (
        <MethodRunner cmd={method} onBack={() => setMethod(null)} />
      ) : (
        <>
          <div className="pal-input">
            <input
              autoFocus
              value={q}
              placeholder="Type a command, an entity (@), or a host method (call)…"
              spellCheck={false}
              onChange={(e) => setQ(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "ArrowDown") {
                  e.preventDefault();
                  setActive((a) => Math.min(items.length - 1, a + 1));
                } else if (e.key === "ArrowUp") {
                  e.preventDefault();
                  setActive((a) => Math.max(0, a - 1));
                } else if (e.key === "Enter") {
                  e.preventDefault();
                  const it = items[active];
                  if (it && !it.disabled) it.run();
                }
              }}
            />
            <span className="pal-hint">
              <kbd className="kbd">&gt;</kbd> commands <kbd className="kbd">@</kbd> entities <kbd className="kbd">call</kbd> methods
            </span>
          </div>
          <div ref={list} className="pal-list">
            {items.length === 0 && <div className="pal-empty">Nothing matches “{q}”.</div>}
            {items.map((it, i) => (
              <div key={it.key}>
                {grouped && (i === 0 || items[i - 1]!.section !== it.section) && <div className="pal-section">{it.section}</div>}
                <div
                  className={cx("pal-item", i === active && "is-active", it.disabled && "is-disabled")}
                  onPointerMove={() => setActive(i)}
                  onClick={() => !it.disabled && it.run()}
                >
                  <span className="pal-icon">{it.icon}</span>
                  <span className="pal-title">
                    <Highlight text={it.title} idx={it.match} />
                  </span>
                  {it.detail && <span className="pal-detail">{it.detail}</span>}
                  {!grouped && <span className="pal-tag">{it.section}</span>}
                  {it.keys && <Kbd keys={it.keys} />}
                  {i === active && <CornerDownLeft size={12} className="pal-enter" />}
                </div>
              </div>
            ))}
          </div>
          <div className="pal-foot">
            <span>
              {allCommands().filter((c) => !c.hidden).length} commands · {catalog.length} host methods · {order.length} entities
            </span>
            <span>↑↓ navigate · ↵ run · esc close</span>
          </div>
        </>
      )}
    </Dialog>
  );
}

function runCommand(c: Command, close: () => void) {
  close();
  // Let the palette unmount (and give focus back) before the command runs.
  requestAnimationFrame(() => void c.run());
}
