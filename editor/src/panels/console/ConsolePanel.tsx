// The console: the host's log (script console.*, errors, host messages) with level filters, search
// and links to the source line, and an input line that evaluates in the game (`debug.eval`, on the
// selected frame while paused).

import { ArrowDownToLine, Ban, ChevronRight, CircleAlert, Info, TriangleAlert, Bug } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { evaluate } from "../../actions/debug";
import { useDebug } from "../../state/debug";
import { useLogs, type ConsoleLevel, type ConsoleLine } from "../../state/logs";
import { useScripts } from "../../state/scripts";
import { IconButton } from "../../ui/Button";
import { VariableRow } from "../../ui/VariableTree";
import { cx } from "../../ui/cx";
import { timeOfDay } from "../../ui/format";
import { PanelShell } from "../../ui/Panel";
import { SearchInput } from "../../ui/SearchInput";
import { Toolbar, ToolbarSeparator } from "../../ui/Toolbar";
import "./console.css";

type Filter = "error" | "warn" | "info" | "debug";

const LEVELS: { level: Filter; label: string; icon: typeof Info }[] = [
  { level: "error", label: "Errors", icon: CircleAlert },
  { level: "warn", label: "Warnings", icon: TriangleAlert },
  { level: "info", label: "Info", icon: Info },
  { level: "debug", label: "Debug", icon: Bug },
];

function bucket(level: ConsoleLevel): Filter {
  if (level === "error") return "error";
  if (level === "warn") return "warn";
  if (level === "debug" || level === "trace") return "debug";
  return "info";
}

const HISTORY_KEY = "aipocket2.editor.console.history";

function Line({ line }: { line: ConsoleLine }) {
  const [open, setOpen] = useState(false);
  if (line.value?.children?.length) {
    return (
      <div className="log-row lvl-result log-tree">
        <span className="log-time">{timeOfDay(line.ts)}</span>
        <span className="log-icon">
          <span className="result-arrow">←</span>
        </span>
        <div className="log-value">
          <VariableRow v={line.value} label={line.value.type} />
        </div>
      </div>
    );
  }
  const Icon = line.level === "error" ? CircleAlert : line.level === "warn" ? TriangleAlert : line.level === "input" ? ChevronRight : line.level === "result" ? null : line.level === "debug" ? Bug : Info;
  return (
    <div className={cx("log-row", `lvl-${line.level}`, open && "is-open")} onClick={() => setOpen(!open)}>
      <span className="log-time" data-tip={line.tick !== undefined ? `tick ${line.tick}` : undefined}>
        {timeOfDay(line.ts)}
      </span>
      <span className="log-icon">{Icon ? <Icon size={12} /> : <span className="result-arrow">←</span>}</span>
      {line.level !== "input" && line.level !== "result" && <span className="log-source">{line.source}</span>}
      <span className="log-msg">{line.message}</span>
      {line.file && (
        <button
          type="button"
          className="log-link"
          onClick={(e) => {
            e.stopPropagation();
            useScripts.getState().openFile(line.file!, line.line);
          }}
        >
          {line.file.split("/").pop()}
          {line.line ? `:${line.line}` : ""}
        </button>
      )}
    </div>
  );
}

export function ConsolePanel() {
  const lines = useLogs((s) => s.lines);
  const clear = useLogs((s) => s.clear);
  const paused = useDebug((s) => s.state.state === "paused");
  const frame = useDebug((s) => s.frame);
  const [shown, setShown] = useState<Set<Filter>>(() => new Set(["error", "warn", "info", "debug"]));
  const [q, setQ] = useState("");
  const [follow, setFollow] = useState(true);
  const [input, setInput] = useState("");
  const [history, setHistory] = useState<string[]>(() => {
    try {
      return JSON.parse(localStorage.getItem(HISTORY_KEY) ?? "[]") as string[];
    } catch {
      return [];
    }
  });
  const [hpos, setHpos] = useState(-1);
  const list = useRef<HTMLDivElement>(null);

  const counts = useMemo(() => {
    const c: Record<Filter, number> = { error: 0, warn: 0, info: 0, debug: 0 };
    for (const l of lines) c[bucket(l.level)]++;
    return c;
  }, [lines]);

  const visible = useMemo(() => {
    const needle = q.toLowerCase();
    const out = lines.filter((l) => shown.has(bucket(l.level)) && (!needle || l.message.toLowerCase().includes(needle) || l.source.toLowerCase().includes(needle)));
    return out.slice(-1500);
  }, [lines, shown, q]);

  useEffect(() => {
    if (follow && list.current) list.current.scrollTop = list.current.scrollHeight;
  }, [visible, follow]);

  const submit = () => {
    const expr = input.trim();
    if (!expr) return;
    const next = [expr, ...history.filter((h) => h !== expr)].slice(0, 50);
    setHistory(next);
    localStorage.setItem(HISTORY_KEY, JSON.stringify(next));
    setHpos(-1);
    setInput("");
    setFollow(true);
    void evaluate(expr);
  };

  const toolbar = (
    <Toolbar>
      <div className="level-chips">
        {LEVELS.map(({ level, label, icon: Icon }) => (
          <button
            key={level}
            type="button"
            className={cx("chip", `chip-${level}`, shown.has(level) && "is-on")}
            data-tip={`${shown.has(level) ? "Hide" : "Show"} ${label.toLowerCase()}`}
            onClick={() => {
              const n = new Set(shown);
              if (n.has(level)) n.delete(level);
              else n.add(level);
              setShown(n);
            }}
          >
            <Icon size={12} />
            <span>{counts[level]}</span>
          </button>
        ))}
      </div>
      <SearchInput value={q} onChange={setQ} placeholder="Filter log" />
      <ToolbarSeparator />
      <IconButton icon={ArrowDownToLine} label="Follow new lines" active={follow} onClick={() => setFollow(!follow)} />
      <IconButton icon={Ban} label="Clear console" onClick={clear} />
    </Toolbar>
  );

  return (
    <PanelShell toolbar={toolbar} scroll={false} className="console">
      <div
        ref={list}
        className="log-list"
        onScroll={(e) => {
          const el = e.currentTarget;
          const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
          if (atBottom !== follow) setFollow(atBottom);
        }}
      >
        {visible.map((l) => (
          <Line key={l.id} line={l} />
        ))}
      </div>
      <div className="console-input">
        <ChevronRight size={14} className="prompt" />
        <input
          value={input}
          spellCheck={false}
          placeholder={paused ? `Evaluate on frame ${frame} (paused): locals, closure, ctx…` : "Evaluates while the debugger holds the game (pause or a breakpoint first)"}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") submit();
            else if (e.key === "ArrowUp" && history.length) {
              e.preventDefault();
              const p = Math.min(history.length - 1, hpos + 1);
              setHpos(p);
              setInput(history[p]!);
            } else if (e.key === "ArrowDown") {
              e.preventDefault();
              const p = Math.max(-1, hpos - 1);
              setHpos(p);
              setInput(p < 0 ? "" : history[p]!);
            }
          }}
        />
        <span className="console-scope">{paused ? `frame ${frame}` : "global"}</span>
      </div>
    </PanelShell>
  );
}
