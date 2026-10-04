// Modal dialogs: a text prompt, the keyboard shortcuts sheet and About.

import { useMemo, useState } from "react";
import { allCommands } from "../commands/registry";
import { host } from "../host/api";
import { useConnection } from "../state/connection";
import { useUi } from "../state/ui";
import { Dialog } from "../ui/Dialog";
import { Kbd } from "../ui/Kbd";
import { SearchInput } from "../ui/SearchInput";

export function PromptDialog() {
  const prompt = useUi((s) => s.prompt);
  const [value, setValue] = useState("");
  const [shownFor, setShownFor] = useState<unknown>(null);
  if (!prompt) return null;
  if (shownFor !== prompt) {
    setShownFor(prompt);
    setValue(prompt.value);
  }
  const done = (v: string | null) => {
    useUi.getState().set({ prompt: null });
    prompt.resolve(v);
  };
  return (
    <Dialog onClose={() => done(null)} className="prompt-dialog">
      <div className="dialog-title">{prompt.title}</div>
      {prompt.label && <label className="dialog-label">{prompt.label}</label>}
      <input
        autoFocus
        className="text-field dialog-input"
        value={value}
        placeholder={prompt.placeholder}
        spellCheck={false}
        onChange={(e) => setValue(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") done(value);
        }}
      />
      <div className="dialog-actions">
        <button type="button" className="btn btn-ghost btn-md" onClick={() => done(null)}>
          <span>Cancel</span>
        </button>
        <button type="button" className="btn btn-primary btn-md" onClick={() => done(value)}>
          <span>{prompt.confirm ?? "OK"}</span>
        </button>
      </div>
    </Dialog>
  );
}

export function ShortcutsDialog() {
  const open = useUi((s) => s.shortcutsOpen);
  const [q, setQ] = useState("");
  const groups = useMemo(() => {
    const out = new Map<string, { title: string; keys: string[] }[]>();
    for (const c of allCommands()) {
      if (!c.keys?.length) continue;
      if (q && !c.title.toLowerCase().includes(q.toLowerCase())) continue;
      const g = out.get(c.category) ?? [];
      g.push({ title: c.title, keys: c.keys });
      out.set(c.category, g);
    }
    return [...out.entries()];
  }, [q, open]);
  if (!open) return null;
  return (
    <Dialog onClose={() => useUi.getState().set({ shortcutsOpen: false })} className="shortcuts-dialog">
      <div className="dialog-title">Keyboard shortcuts</div>
      <SearchInput value={q} onChange={setQ} placeholder="Filter shortcuts" />
      <div className="shortcut-grid">
        {groups.map(([cat, list]) => (
          <div key={cat} className="shortcut-group">
            <div className="shortcut-cat">{cat}</div>
            {list.map((s) => (
              <div key={s.title} className="shortcut-row">
                <span>{s.title}</span>
                <span className="shortcut-keys">
                  {s.keys.map((k) => (
                    <Kbd key={k} keys={k} />
                  ))}
                </span>
              </div>
            ))}
          </div>
        ))}
      </div>
      <div className="dialog-note">Viewport: right-drag or Alt+drag orbits, middle-drag or Shift+right-drag pans, wheel zooms, drag on empty space selects a rectangle.</div>
    </Dialog>
  );
}

export function AboutDialog() {
  const open = useUi((s) => s.aboutOpen);
  const project = useConnection((s) => s.project);
  const catalog = useConnection((s) => s.catalog);
  const viewport = useConnection((s) => s.viewport);
  const groups = useConnection((s) => s.editGroups);
  if (!open) return null;
  return (
    <Dialog onClose={() => useUi.getState().set({ aboutOpen: false })} className="about-dialog">
      <div className="dialog-title">Pocket3D Editor</div>
      <p className="muted">The aipocket2 web editor: a client of the host protocol (docs/spec/host-protocol.md), with no private channel. Everything here an agent can do over MCP.</p>
      <dl className="about-list">
        <dt>Host</dt>
        <dd className="mono">{host.endpoints.label}{project?.host ? ` (${project.host})` : ""}</dd>
        <dt>Project</dt>
        <dd>{project ? `${project.name} · ${project.root}` : "—"}</dd>
        <dt>Catalog</dt>
        <dd>{catalog.length} commands</dd>
        <dt>Viewport</dt>
        <dd>{viewport === "wasm" ? "engine renderer (wasm, WebGPU)" : "fallback view (Canvas 2D)"}</dd>
        <dt>Live drags</dt>
        <dd>{groups ? "streamed as grouped world.edit (one undo entry)" : "previewed locally, sent on release"}</dd>
        <dt>Build</dt>
        <dd className="mono">React 19 · dockview · Monaco · Vite</dd>
      </dl>
    </Dialog>
  );
}
