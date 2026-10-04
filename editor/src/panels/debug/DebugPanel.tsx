// The debugger: the pause state and controls (F5 continue, F10 over, F11 into, Shift+F11 out), the
// call stack with TypeScript locations, scopes and variables, watch expressions, line breakpoints
// with conditions, and the engine's data breakpoints. Driven by `debug` events and `debug.*`.

import { ArrowDownToLine, ArrowRightToLine, ArrowUpFromLine, Bug, CirclePause, CirclePlay, Database, Trash, Unplug } from "lucide-react";
import { useState } from "react";
import {
  addWatch,
  clearAllBreakpoints,
  isScriptFile,
  pauseScripts,
  refreshWatches,
  removeBreakpoint,
  removeDataWatch,
  removeWatch,
  resume,
  setBreakpointCondition,
  step,
} from "../../actions/debug";
import { useDebug } from "../../state/debug";
import { useScripts } from "../../state/scripts";
import { useSession } from "../../state/session";
import { entityName } from "../../state/world";
import { IconButton } from "../../ui/Button";
import { cx } from "../../ui/cx";
import { PanelShell, Section } from "../../ui/Panel";
import { Spacer, Toolbar, ToolbarSeparator } from "../../ui/Toolbar";
import { VariableRow } from "../../ui/VariableTree";
import "./debug.css";

function useOpen(key: string, initial = true): [boolean, () => void] {
  const [open, setOpen] = useState(() => {
    const v = localStorage.getItem(`aipocket2.editor.debug.${key}`);
    return v === null ? initial : v === "1";
  });
  return [
    open,
    () => {
      localStorage.setItem(`aipocket2.editor.debug.${key}`, open ? "0" : "1");
      setOpen(!open);
    },
  ];
}

export function DebugPanel() {
  const { state, frame, breakpoints, watches, dataWatches, setFrame } = useDebug();
  const mode = useSession((s) => s.status?.mode);
  const paused = state.state === "paused";
  const frames = state.frames ?? [];
  const current = frames.find((f) => f.id === frame) ?? frames[0];
  const [stackOpen, toggleStack] = useOpen("stack");
  const [varsOpen, toggleVars] = useOpen("vars");
  const [watchOpen, toggleWatch] = useOpen("watch");
  const [bpOpen, toggleBp] = useOpen("bp");
  const [dataOpen, toggleData] = useOpen("data");
  const [newWatch, setNewWatch] = useState("");
  const [editingCond, setEditingCond] = useState<number | null>(null);

  const toolbar = (
    <Toolbar>
      {paused ? (
        <IconButton icon={CirclePlay} label="Continue" keys="F5" tone="play" onClick={() => void resume()} />
      ) : (
        <IconButton icon={CirclePause} label="Pause scripts" keys="F6" disabled={mode !== "play"} onClick={() => void pauseScripts()} />
      )}
      <ToolbarSeparator />
      <IconButton icon={ArrowRightToLine} label="Step over" keys="F10" disabled={!paused} onClick={() => void step("over")} />
      <IconButton icon={ArrowDownToLine} label="Step into" keys="F11" disabled={!paused} onClick={() => void step("into")} />
      <IconButton icon={ArrowUpFromLine} label="Step out" keys="Shift+F11" disabled={!paused} onClick={() => void step("out")} />
      <Spacer />
      <span className={cx("debug-state", paused && "is-paused")}>
        {paused ? `Paused · ${state.reason ?? ""}` : mode === "play" ? "Running" : "Not running (Edit)"}
      </span>
    </Toolbar>
  );

  return (
    <PanelShell toolbar={toolbar} scroll={false} className="debug-panel">
      {paused && (
        <div className="pause-banner">
          <Bug size={14} />
          <span className="pause-title">Paused on {state.reason ?? "pause"}</span>
          {state.tick !== undefined && <span className="mono">tick {state.tick}</span>}
          {state.system && <span className="mono">{state.system}</span>}
          {state.location && state.location.line > 0 && (
            <button type="button" className="link mono" onClick={() => useScripts.getState().openFile(state.location!.file, state.location!.line)}>
              {state.location.file}:{state.location.line}
            </button>
          )}
          {state.detail && <span className="pause-detail">{state.detail}</span>}
        </div>
      )}
      <div className="debug-body">
        <div className="debug-grid">
          <div className="debug-col">
            <Section title="Call Stack" open={stackOpen} onToggle={toggleStack} count={frames.length || undefined}>
              {!paused ? (
                <div className="section-empty">Not paused. Set a breakpoint in a script (gutter or F9) and press Play.</div>
              ) : (
                frames.map((f) => (
                  <div
                    key={f.id}
                    className={cx("frame-row", f.id === (current?.id ?? 0) && "is-current", !isScriptFile(f.file) && "is-host")}
                    onClick={() => {
                      setFrame(f.id);
                      void refreshWatches();
                      if (isScriptFile(f.file)) useScripts.getState().openFile(f.file, f.line);
                    }}
                  >
                    <span className="frame-name">{f.name}</span>
                    <span className="frame-loc mono">{f.line > 0 ? `${f.file.split("/").pop()}:${f.line}` : f.file}</span>
                  </div>
                ))
              )}
            </Section>
            <Section
              title="Breakpoints"
              open={bpOpen}
              onToggle={toggleBp}
              count={breakpoints.length || undefined}
              actions={breakpoints.length > 0 && <IconButton icon={Trash} size="sm" label="Remove all breakpoints" onClick={() => void clearAllBreakpoints()} />}
            >
              {breakpoints.length === 0 && <div className="section-empty">Click the gutter of a script, or press F9.</div>}
              {breakpoints.map((b) => (
                <div key={b.id} className="bp-row">
                  <span className={cx("bp-dot", b.condition && "cond", b.verified === false && "unverified")} />
                  <button type="button" className="link mono" onClick={() => useScripts.getState().openFile(b.file, b.line)}>
                    {b.file.split("/").pop()}:{b.line}
                  </button>
                  {editingCond === b.id ? (
                    <input
                      autoFocus
                      className="cond-input"
                      defaultValue={b.condition ?? ""}
                      placeholder="condition, e.g. speed > 2"
                      onKeyDown={(e) => {
                        if (e.key === "Enter") e.currentTarget.blur();
                        if (e.key === "Escape") setEditingCond(null);
                      }}
                      onBlur={(e) => {
                        setEditingCond(null);
                        if (e.target.value !== (b.condition ?? "")) void setBreakpointCondition(b.id, e.target.value);
                      }}
                    />
                  ) : (
                    <span className="bp-cond-text" onClick={() => setEditingCond(b.id)} data-tip="Click to set a condition">
                      {b.condition ? `when ${b.condition}` : "always"}
                    </span>
                  )}
                  {b.hits !== undefined && b.hits > 0 && <span className="badge">{b.hits}</span>}
                  {b.verified === false && <span className="bp-note" data-tip="Not inside a system's code; it will not be hit">unbound</span>}
                  <button type="button" className="var-remove" aria-label="Remove" onClick={() => void removeBreakpoint(b.id)}>
                    ×
                  </button>
                </div>
              ))}
            </Section>
            <Section title="Data Breakpoints" open={dataOpen} onToggle={toggleData} count={dataWatches.length || undefined}>
              {dataWatches.length === 0 && (
                <div className="section-empty">
                  <Database size={12} /> Pause when a system writes a component field: inspector card menu → Break When Written.
                </div>
              )}
              {dataWatches.map((w) => (
                <div key={w.id} className="bp-row">
                  <Unplug size={12} className="data-icon" />
                  <span className="mono">
                    {entityName(w.entity)}.{w.component}
                    {w.field ? `.${w.field}` : ""}
                  </span>
                  <span className="bp-cond-text">on write</span>
                  <button type="button" className="var-remove" aria-label="Remove" onClick={() => void removeDataWatch(w.id)}>
                    ×
                  </button>
                </div>
              ))}
            </Section>
          </div>
          <div className="debug-col">
            <Section title="Variables" open={varsOpen} onToggle={toggleVars}>
              {!paused || !current?.scopes?.length ? (
                <div className="section-empty">{paused ? "No scopes for this frame." : "Variables show when the game pauses."}</div>
              ) : (
                current.scopes.map((s) => (
                  <div key={s.name} className="scope">
                    <div className="scope-name">{s.name}</div>
                    {s.variables.map((v, i) => (
                      <VariableRow key={`${v.name}-${i}`} v={v} />
                    ))}
                  </div>
                ))
              )}
            </Section>
          </div>
          <div className="debug-col">
            <Section title="Watch" open={watchOpen} onToggle={toggleWatch} count={watches.length || undefined}>
              {watches.map((w) =>
                w.value ? (
                  <VariableRow key={w.id} v={w.value} label={w.expr} onRemove={() => removeWatch(w.id)} />
                ) : (
                  <div key={w.id} className="var-row">
                    <span className="var-twisty" />
                    <span className="var-name">{w.expr}</span>
                    <span className="var-sep">:</span>
                    <span className={cx("var-value", w.error ? "v-err" : "v-null")}>{w.error ?? "not evaluated"}</span>
                    <button type="button" className="var-remove" onClick={() => removeWatch(w.id)}>
                      ×
                    </button>
                  </div>
                ),
              )}
              <input
                className="watch-input"
                placeholder="Add expression (e.g. speed * 2, ctx.tick)"
                value={newWatch}
                onChange={(e) => setNewWatch(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    addWatch(newWatch);
                    setNewWatch("");
                  }
                }}
              />
            </Section>
          </div>
        </div>
      </div>
    </PanelShell>
  );
}
