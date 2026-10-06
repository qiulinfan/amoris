// The script editor: Monaco with one model per project script (so imports between scripts resolve
// in the TypeScript worker), tabs, save as `scripts.write` + `scripts.apply`, the host's diagnostics
// as markers next to the worker's own, a breakpoint gutter (`debug.breakpoints.set`) and the paused
// line while the debugger holds the game.

import { Circle, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { api } from "../../host/api";
import type { Diagnostic } from "../../host/protocol";
import { registerBreakpointToggler, registerScriptSaver, saveScript } from "../../actions/scripts";
import { toggleBreakpoint } from "../../actions/debug";
import { reportError } from "../../actions/report";
import { useDebug } from "../../state/debug";
import { useScripts } from "../../state/scripts";
import { cx } from "../../ui/cx";
import { monaco, pathOf, sdkLabel, uriOf } from "./monaco";

type Editor = ReturnType<typeof monaco.editor.create>;
type Model = ReturnType<typeof monaco.editor.createModel>;

/** The text each file had on the host when last read or saved. */
const hostText = new Map<string, string>();
const viewStates = new Map<string, unknown>();
const saving = new Set<string>();

function severity(s: Diagnostic["severity"]) {
  return s === "error" ? monaco.MarkerSeverity.Error : s === "warning" ? monaco.MarkerSeverity.Warning : s === "info" ? monaco.MarkerSeverity.Info : monaco.MarkerSeverity.Hint;
}

async function ensureModel(path: string): Promise<Model | null> {
  const uri = uriOf(path);
  const existing = monaco.editor.getModel(uri);
  if (existing) return existing;
  try {
    const text = await api.scripts.read(path);
    hostText.set(path, text);
    return monaco.editor.getModel(uri) ?? monaco.editor.createModel(text, "typescript", uri);
  } catch (e) {
    reportError(e, `Open ${path}`);
    return null;
  }
}

export default function CodeEditor() {
  const host = useRef<HTMLDivElement>(null);
  const editorRef = useRef<Editor | null>(null);
  const { files, open, active, dirty, diagnostics, reveal, types } = useScripts();
  const breakpoints = useDebug((s) => s.breakpoints);
  const debugState = useDebug((s) => s.state);
  const [cursor, setCursor] = useState({ line: 1, column: 1 });
  // The shown model's markers (the TypeScript worker's and the host's), counted in the status bar.
  const [counts, setCounts] = useState({ errors: 0, warnings: 0 });
  const bpDecorations = useRef<ReturnType<Editor["createDecorationsCollection"]> | null>(null);
  const pauseDecorations = useRef<ReturnType<Editor["createDecorationsCollection"]> | null>(null);
  const hoverDecorations = useRef<ReturnType<Editor["createDecorationsCollection"]> | null>(null);

  // Create the editor once.
  useEffect(() => {
    // Hovers and suggestions live in a node on the body: the dock's panels clip anything inside them,
    // and a type error's hover is often wider than what is left of the editor.
    const overflow = document.createElement("div");
    overflow.className = "monaco-editor";
    document.body.appendChild(overflow);
    const editor = monaco.editor.create(host.current!, {
      theme: "pocket-dark",
      fontFamily: "'JetBrains Mono Variable', 'JetBrains Mono', ui-monospace, Menlo, monospace",
      fontSize: 13,
      lineHeight: 20,
      fontLigatures: true,
      glyphMargin: true,
      automaticLayout: true,
      minimap: { enabled: true, renderCharacters: false, scale: 1, maxColumn: 80 },
      scrollBeyondLastLine: false,
      smoothScrolling: true,
      cursorSmoothCaretAnimation: "on",
      cursorBlinking: "smooth",
      renderLineHighlight: "all",
      padding: { top: 8, bottom: 8 },
      bracketPairColorization: { enabled: true },
      guides: { bracketPairs: "active", indentation: true },
      fixedOverflowWidgets: true,
      overflowWidgetsDomNode: overflow,
      tabSize: 4,
      stickyScroll: { enabled: true },
      model: null,
    });
    editorRef.current = editor;
    bpDecorations.current = editor.createDecorationsCollection();
    pauseDecorations.current = editor.createDecorationsCollection();
    hoverDecorations.current = editor.createDecorationsCollection();

    editor.onDidChangeCursorPosition((e) => setCursor({ line: e.position.lineNumber, column: e.position.column }));
    editor.onDidChangeModelContent(() => {
      const model = editor.getModel();
      if (!model) return;
      const path = pathOf(model.uri);
      const isDirty = model.getValue() !== hostText.get(path);
      if (useScripts.getState().dirty[path] !== isDirty) useScripts.getState().setDirty(path, isDirty);
    });
    editor.onMouseDown((e) => {
      if (e.target.type !== monaco.editor.MouseTargetType.GUTTER_GLYPH_MARGIN) return;
      const line = e.target.position?.lineNumber;
      const model = editor.getModel();
      if (line && model) void toggleBreakpoint(pathOf(model.uri), line);
    });
    editor.onMouseMove((e) => {
      const line = e.target.type === monaco.editor.MouseTargetType.GUTTER_GLYPH_MARGIN ? e.target.position?.lineNumber : undefined;
      hoverDecorations.current?.set(line ? [{ range: new monaco.Range(line, 1, line, 1), options: { glyphMarginClassName: "bp-ghost" } }] : []);
    });
    editor.onMouseLeave(() => hoverDecorations.current?.set([]));
    const count = () => {
      const model = editor.getModel();
      const ms = model ? monaco.editor.getModelMarkers({ resource: model.uri }) : [];
      setCounts({
        errors: ms.filter((m) => m.severity === monaco.MarkerSeverity.Error).length,
        warnings: ms.filter((m) => m.severity === monaco.MarkerSeverity.Warning).length,
      });
    };
    const markersSub = monaco.editor.onDidChangeMarkers(count);
    editor.onDidChangeModel(count);

    const save = () => {
      const model = editor.getModel();
      if (!model) return;
      const path = pathOf(model.uri);
      if (saving.has(path)) return;
      const text = model.getValue();
      saving.add(path);
      void saveScript(path, text).then((saved) => {
        if (!saved) return;
        hostText.set(path, text);
        useScripts.getState().setDirty(path, model.isDisposed() || model.getValue() !== text);
      }).finally(() => saving.delete(path));
    };
    registerScriptSaver(save);
    registerBreakpointToggler(() => {
      const model = editor.getModel();
      const pos = editor.getPosition();
      if (model && pos) void toggleBreakpoint(pathOf(model.uri), pos.lineNumber);
    });
    return () => {
      markersSub.dispose();
      registerScriptSaver(null);
      registerBreakpointToggler(null);
      editor.dispose();
      overflow.remove();
      editorRef.current = null;
    };
  }, []);

  // Models for every script, so cross-file imports type-check.
  useEffect(() => {
    for (const f of files) void ensureModel(f.path);
    if (!useScripts.getState().active && files.length) {
      const main = files.find((f) => f.path.endsWith("main.ts")) ?? files[0]!;
      useScripts.getState().openFile(main.path);
    }
  }, [files]);

  // The active file.
  useEffect(() => {
    const editor = editorRef.current;
    if (!editor || !active) {
      editor?.setModel(null);
      return;
    }
    let cancelled = false;
    void ensureModel(active).then((model) => {
      if (cancelled || !model) return;
      const prev = editor.getModel();
      if (prev === model) return;
      if (prev) viewStates.set(pathOf(prev.uri), editor.saveViewState());
      editor.setModel(model);
      const vs = viewStates.get(active);
      if (vs) editor.restoreViewState(vs as Parameters<Editor["restoreViewState"]>[0]);
    });
    return () => {
      cancelled = true;
    };
  }, [active]);

  // Reveal requests (console links, the debugger, events).
  useEffect(() => {
    if (!reveal) return;
    const editor = editorRef.current;
    if (!editor) return;
    void ensureModel(reveal.path).then((model) => {
      if (!model) return;
      if (editor.getModel() !== model) editor.setModel(model);
      editor.setPosition({ lineNumber: reveal.line, column: 1 });
      // After the panel has been laid out (it may just have been shown).
      requestAnimationFrame(() => {
        editor.layout();
        editor.revealLineInCenter(reveal.line);
        editor.focus();
      });
    });
  }, [reveal]);

  // Host diagnostics as markers. Once the worker has the project's declarations it checks against the
  // same files `tsc` did, live, so the host's `tsc` findings (stale after the next keystroke) are
  // left to it.
  useEffect(() => {
    const live = !!types?.text;
    for (const [path, list] of Object.entries(diagnostics)) {
      const model = monaco.editor.getModel(uriOf(path));
      if (!model) continue;
      monaco.editor.setModelMarkers(
        model,
        "pocket",
        list.filter((d) => !(live && d.source === "tsc")).map((d) => ({
          severity: severity(d.severity),
          message: d.message,
          code: d.code !== undefined ? String(d.code) : undefined,
          source: "pocket",
          // A finding with no place (a broken tsconfig.json) marks the first line.
          startLineNumber: d.line || 1,
          startColumn: d.column || 1,
          endLineNumber: d.end_line ?? (d.line || 1),
          endColumn: d.end_column ?? model.getLineMaxColumn(d.line || 1),
        })),
      );
    }
  }, [diagnostics, files, types]);

  // Breakpoints and the paused line of the shown file.
  const shown = editorRef.current?.getModel();
  const shownPath = shown ? pathOf(shown.uri) : active;
  useEffect(() => {
    const path = shownPath;
    bpDecorations.current?.set(
      breakpoints
        .filter((b) => b.file === path)
        .map((b) => ({
          range: new monaco.Range(b.line, 1, b.line, 1),
          options: {
            glyphMarginClassName: cx("bp", b.condition && "bp-cond", b.verified === false && "bp-unverified"),
            glyphMarginHoverMessage: { value: b.condition ? `Breakpoint when \`${b.condition}\`` : "Breakpoint" },
            stickiness: monaco.editor.TrackedRangeStickiness.NeverGrowsWhenTypingAtEdges,
          },
        })),
    );
    const loc = debugState.state === "paused" ? debugState.location : undefined;
    if (loc && loc.file === path && loc.line > 0) {
      const editor = editorRef.current;
      requestAnimationFrame(() => editor?.revealLineInCenterIfOutsideViewport(loc.line));
    }
    pauseDecorations.current?.set(
      loc && loc.file === path
        ? [
            {
              range: new monaco.Range(loc.line, 1, loc.line, 1),
              options: { isWholeLine: true, className: "paused-line", glyphMarginClassName: "paused-glyph", overviewRuler: { color: "#e5a53a", position: monaco.editor.OverviewRulerLane.Full } },
            },
          ]
        : [],
    );
  }, [breakpoints, debugState, shownPath, active]);

  const { errors, warnings } = counts;

  return (
    <div className="code-editor">
      <div className="code-tabs" role="tablist">
        {open.map((p) => {
          const diags = diagnostics[p] ?? [];
          const err = diags.some((d) => d.severity === "error");
          return (
            <div
              key={p}
              role="tab"
              aria-selected={p === active}
              className={cx("code-tab", p === active && "is-active", err && "has-error")}
              onClick={() => useScripts.getState().setActive(p)}
              onAuxClick={(e) => e.button === 1 && useScripts.getState().closeFile(p)}
              data-tip={p}
            >
              <span className="code-tab-name">{p.split("/").pop()}</span>
              <button
                type="button"
                className={cx("code-tab-close", dirty[p] && "is-dirty")}
                aria-label="Close"
                onClick={(e) => {
                  e.stopPropagation();
                  useScripts.getState().closeFile(p);
                }}
              >
                {dirty[p] ? <Circle size={8} fill="currentColor" /> : <X size={12} />}
              </button>
            </div>
          );
        })}
      </div>
      <div ref={host} className="code-host" />
      <div className="code-status">
        <span>
          Ln {cursor.line}, Col {cursor.column}
        </span>
        <span>TypeScript</span>
        <span data-tip="Where the `pocket` module's types come from (scripts.types)">SDK: {sdkLabel(types)}</span>
        <span className={cx(errors > 0 && "err")}>{errors} errors</span>
        <span className={cx(warnings > 0 && "warn")}>{warnings} warnings</span>
        {active && dirty[active] && <span className="warn">unsaved</span>}
      </div>
    </div>
  );
}
