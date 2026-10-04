// The debugger: the pause state and controls (F5 continue, F10 over, F11 into, Shift+F11 out), pause
// on exceptions, the call stack with TypeScript locations, the frame's scopes as expandable trees
// whose values can be set, watch expressions, line breakpoints with conditions, and the engine's
// data breakpoints. Driven by `debug` events and `debug.*` (docs/spec/debugger.md 7).

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
  setExceptionMode,
  setValue,
  step,
} from "../../actions/debug";
import type { DebugState, ExceptionMode, SourceLocation } from "../../host/protocol";
import { useDebug } from "../../state/debug";
import { useScripts } from "../../state/scripts";
import { useSession } from "../../state/session";
import { entityName } from "../../state/world";
import { IconButton } from "../../ui/Button";
import { cx } from "../../ui/cx";
import { SelectField } from "../../ui/fields/SelectField";
import { PanelShell, Section } from "../../ui/Panel";
import { Spacer, Toolbar, ToolbarSeparator } from "../../ui/Toolbar";
import { VariableRow } from "../../ui/VariableTree";
import "./debug.css";

function useOpen(key: string, initial = true): [boolean, () => void] {
  const [open, setOpen] = useState(() => {
    try {
      const v = localStorage.getItem(`aipocket2.editor.debug.${key}`);
      return v === null ? initial : v === "1";
    } catch {
      return initial;
    }
  });
  return [
    open,
    () => {
      try {
        localStorage.setItem(`aipocket2.editor.debug.${key}`, open ? "0" : "1");
      } catch {
        // Storage refused: the section still toggles.
      }
      setOpen(!open);
    },
  ];
}

const REASONS: Record<string, string> = {
  breakpoint: "breakpoint",
  step: "step",
  pause: "pause request",
  debugger_statement: "debugger statement",
  exception: "exception",
  data_breakpoint: "data breakpoint",
};

const EXCEPTION_MODES: { value: ExceptionMode; label: string; doc: string }[] = [
  { value: "none", label: "Exceptions: off", doc: "Never pause on exceptions" },
  { value: "uncaught", label: "Exceptions: uncaught", doc: "Pause where an exception no try catches is thrown" },
  { value: "all", label: "Exceptions: all", doc: "Pause at every throw" },
];

function at(loc: SourceLocation | null | undefined): string {
  return loc ? `${loc.file.split("/").pop()}:${loc.line}` : "";
}

/** A watched value: one slot unwrapped, numbers to six decimals. */
function show(v: unknown): string {
  if (Array.isArray(v) && v.length === 1) return show(v[0]);
  if (typeof v === "number") return Number.isInteger(v) ? String(v) : String(Math.round(v * 1e6) / 1e6);
  if (Array.isArray(v)) return `[${v.map(show).join(", ")}]`;
  return typeof v === "string" ? v : JSON.stringify(v);
}

/** Why it stopped, beyond the reason: the data breakpoint's write or the exception. */
function pauseDetail(state: DebugState): string | null {
  if (state.data) {
    const d = state.data;
    const what = `${entityName(d.entity)}.${d.component}${d.field ? `.${d.field}` : ""}`;
    const where = d.written_at ? ` (written at ${at(d.written_at)}${d.after_return ? ", seen as run returned" : ""})` : "";
    return `${what}: ${show(d.before)} → ${show(d.after)}${where}`;
  }
  if (state.exception) return `${state.exception.caught ? "caught" : "uncaught"}: ${state.exception.text ?? ""}`;
  return null;
}

export function DebugPanel() {
  const { state, frame, breakpoints, watches, dataWatches, exceptions, setFrame } = useDebug();
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
  const [editingCond, setEditingCond] = useState<string | null>(null);
  const detail = paused ? pauseDetail(state) : null;
  const canEdit = paused && current && !current.returned;

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
      <ToolbarSeparator />
      <div className="debug-exceptions">
        <SelectField value={exceptions} options={EXCEPTION_MODES} onChange={(m) => void setExceptionMode(m as ExceptionMode)} />
      </div>
      <Spacer />
      <span className={cx("debug-state", paused && "is-paused")}>
        {paused ? `Paused · ${REASONS[state.reason ?? ""] ?? state.reason ?? ""}` : mode === "play" ? "Running" : "Edit (step or Play to run scripts)"}
      </span>
    </Toolbar>
  );

  return (
    <PanelShell toolbar={toolbar} scroll={false} className="debug-panel">
      {paused && (
        <div className="pause-banner">
          <Bug size={14} />
          <span className="pause-title">Paused on {REASONS[state.reason ?? ""] ?? state.reason ?? "pause"}</span>
          {state.tick !== undefined && <span className="mono">tick {state.tick}</span>}
          {state.system && <span className="mono">system {state.system}</span>}
          {state.location && state.location.line > 0 && (
            <button type="button" className="link mono" onClick={() => useScripts.getState().openFile(state.location!.file, state.location!.line)}>
              {state.location.file}:{state.location.line}
              {state.location.column ? `:${state.location.column}` : ""}
            </button>
          )}
          {detail && <span className="pause-detail">{detail}</span>}
        </div>
      )}
      <div className="debug-body">
        <div className="debug-grid">
          <div className="debug-col">
            <Section title="Call Stack" open={stackOpen} onToggle={toggleStack} count={frames.length || undefined}>
              {!paused ? (
                <div className="section-empty">Not paused. Set a breakpoint in a script (gutter or F9), then Play or Step.</div>
              ) : (
                frames.map((f) => (
                  <div
                    key={f.id}
                    className={cx("frame-row", f.id === (current?.id ?? 0) && "is-current", !isScriptFile(f.file) && "is-host")}
                    data-tip={`${f.file}:${f.line}${f.column ? `:${f.column}` : ""}${f.generated ? " (generated JavaScript)" : ""}`}
                    onClick={() => {
                      setFrame(f.id);
                      void refreshWatches();
                      if (isScriptFile(f.file)) useScripts.getState().openFile(f.file, f.line);
                    }}
                  >
                    <span className="frame-name">{f.name}</span>
                    {f.returned && <span className="frame-tag">returned</span>}
                    <span className="frame-loc mono">{f.line > 0 ? at(f) : f.file}</span>
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
                <div key={b.id} className={cx("bp-row", state.hitBreakpoints?.includes(b.id) && paused && "is-hit")}>
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
                    <span
                      className="bp-cond-text"
                      onClick={() => b.owner !== "cdp" && setEditingCond(b.id)}
                      data-tip={b.log !== undefined ? "A logpoint: click to set its condition (it keeps logging)" : "Click to set a condition"}
                    >
                      {b.log !== undefined
                        ? `log ${b.log}${b.condition ? ` when ${b.condition}` : ""}`
                        : b.condition
                          ? `when ${b.condition}`
                          : "always"}
                    </span>
                  )}
                  {b.owner === "cdp" && <span className="bp-note" data-tip="Set by a CDP client (Chrome DevTools, VS Code)">CDP</span>}
                  {b.hits !== undefined && b.hits > 0 && <span className="badge">{b.hits}</span>}
                  {b.verified === false && <span className="bp-note" data-tip="No running script has code there yet">unbound</span>}
                  <button type="button" className="var-remove" aria-label="Remove" onClick={() => void removeBreakpoint(b.id)}>
                    ×
                  </button>
                </div>
              ))}
            </Section>
            <Section title="Data Breakpoints" open={dataOpen} onToggle={toggleData} count={dataWatches.length || undefined}>
              {dataWatches.length === 0 && (
                <div className="section-empty">
                  <Database size={12} /> Pause when a script system writes a component field: inspector card menu → Break When Written.
                </div>
              )}
              {dataWatches.map((w) => (
                <div key={w.id} className={cx("bp-row", paused && state.data?.watch === w.id && "is-hit")}>
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
              {!paused || !current ? (
                <div className="section-empty">Variables show when the game pauses.</div>
              ) : current.returned ? (
                <div className="section-empty">This frame has returned: its position only (the write was seen as run returned).</div>
              ) : (
                current.scopes.map((s) => (
                  <div key={s.name} className="scope">
                    <div className="scope-name">{s.name}</div>
                    {s.variables.length === 0 && <div className="section-empty">none</div>}
                    {s.variables.map((v, i) => (
                      <VariableRow
                        key={`${v.name}-${i}`}
                        v={v}
                        onEdit={canEdit ? (x, text) => setValue(current.id, x.path!, text) : undefined}
                      />
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
                    <span className={cx("var-value", w.error ? "v-err" : "v-null")}>{w.error ?? (paused ? "…" : "evaluated when paused")}</span>
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
