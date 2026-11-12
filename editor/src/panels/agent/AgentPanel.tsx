// What agents do on this host: its agent sessions (`agent.session.list`) and the stream of what
// they called (`agent.session.events`, then `agent` pushes), read from the host on connect, so an
// editor that opens later shows what happened before it came (server.md 3.5). The New session form
// starts one of the agent programs the process owner registered (`amoris serve --agents`,
// `agent.session.start`, server.md 3.7): the editor picks a program by name, never a command or a
// credential.

import { ArrowRight, Bot, Check, ChevronRight, ClipboardCopy, MessageSquare, Play, Plug, Square, Unplug, X } from "lucide-react";
import { useMemo, useState } from "react";
import { startAgent, stopAgent } from "../../actions/agent";
import { host } from "../../host/api";
import type { AgentSession } from "../../host/protocol";
import { useAgent, type AgentLine } from "../../state/agent";
import { useConnection } from "../../state/connection";
import { useUi } from "../../state/ui";
import { Button, IconButton } from "../../ui/Button";
import { cx } from "../../ui/cx";
import { ago, timeOfDay } from "../../ui/format";
import { EmptyState, PanelShell } from "../../ui/Panel";
import { SelectField } from "../../ui/fields/SelectField";
import { Spacer, Toolbar } from "../../ui/Toolbar";
import "./agent.css";

function mcpUrl(): string {
  const base = host.endpoints.http || window.location.origin;
  return `${base}/mcp`;
}

/** Why the form has nothing to start. */
const NO_PROGRAMS =
  "This host has no agent programs. The process owner registers them in a file outside the project: amoris serve <project> --agents <file>. An agent connected with amoris mcp or the MCP endpoint appears in the list all the same.";

/** How a session reached the host, and whose calls it makes. */
function describe(row: AgentSession): string {
  const who = row.label || (row.transport === "api" ? "calls made with no session" : "an unnamed client");
  const as = row.role === "player" ? `plays ${row.seat ?? "a seat"}` : "developer";
  return `${who} (${row.transport}, ${as})`;
}

function Line({ l }: { l: AgentLine }) {
  const Icon =
    l.kind === "opened" ? Plug : l.kind === "closed" ? Unplug : l.kind === "call" ? ArrowRight : l.kind === "message" ? MessageSquare : l.ok === false ? X : Check;
  const text =
    l.kind === "opened"
      ? `opened: ${l.row ? describe(l.row) : l.session}`
      : l.kind === "closed"
        ? `closed after ${l.row?.calls ?? 0} ${l.row?.calls === 1 ? "call" : "calls"}${l.row?.failed_calls ? `, ${l.row.failed_calls} failed` : ""}`
        : l.summary;
  return (
    <div className={cx("ag-line", `ag-${l.kind}`, l.ok === false && "is-failed", l.stream && `is-${l.stream}`)}>
      <span className="ag-time mono">{timeOfDay(l.ts)}</span>
      <span className="ag-icon">
        <Icon size={12} />
      </span>
      {l.method && <span className="ag-method mono">{l.method}</span>}
      <span className="ag-summary">{text}</span>
    </div>
  );
}

function message(e: unknown): string {
  return e instanceof Error ? e.message : typeof e === "object" && e && "message" in e ? String((e as { message: unknown }).message) : String(e);
}

/** Starts a registered agent program with a goal, and stops the selected one. */
function NewSession({ selected }: { selected: AgentSession | undefined }) {
  const catalog = useConnection((s) => s.catalog);
  const programs = useAgent((s) => s.programs);
  const supported = catalog.some((c) => c.name === "agent.session.start") && programs.length > 0;
  const [picked, setPicked] = useState("");
  const [goal, setGoal] = useState("Make every crate reachable on the sloop's course, then sail the course and report the tally.");
  const [seat, setSeat] = useState("");
  const [busy, setBusy] = useState(false);
  const [open, setOpen] = useState(() => localStorage.getItem("amoris.editor.agent.new") === "1");
  const agent = programs.some((p) => p.name === picked) ? picked : (programs[0]?.name ?? "");
  const stoppable = selected && selected.transport === "launch" && selected.live ? selected : undefined;
  const toggle = () => {
    localStorage.setItem("amoris.editor.agent.new", open ? "0" : "1");
    setOpen(!open);
  };
  const start = async () => {
    setBusy(true);
    try {
      const row = await startAgent({ agent, goal, seat: seat.trim() || undefined });
      useUi.getState().toast({ kind: "success", title: "Agent started", body: `${row.label} as ${row.id}` });
    } catch (e) {
      useUi.getState().toast({ kind: "error", title: "The agent did not start", body: message(e) });
    } finally {
      setBusy(false);
    }
  };
  const stop = async () => {
    if (!stoppable) return;
    try {
      await stopAgent(stoppable.id);
    } catch (e) {
      useUi.getState().toast({ kind: "error", title: "Stop", body: message(e) });
    }
  };
  if (!open) {
    return (
      <button type="button" className="ag-new ag-new-collapsed" onClick={toggle}>
        <Bot size={15} />
        <span>New agent session…</span>
        {!supported && <span className="ag-soon">none registered</span>}
        <ChevronRight size={14} className="ag-chev" />
      </button>
    );
  }
  return (
    <div className="ag-new">
      <div className="ag-new-head" onClick={toggle}>
        <Bot size={15} />
        <span>New agent session</span>
        {!supported && (
          <span className="ag-soon" data-tip={NO_PROGRAMS}>
            none registered
          </span>
        )}
        <ChevronRight size={14} className="ag-chev is-open" />
      </div>
      <div className="ag-form">
        <label>Agent</label>
        {supported ? (
          <SelectField value={agent} onChange={setPicked} options={programs.map((p) => ({ value: p.name, label: p.label }))} />
        ) : (
          <span className="ag-none">{NO_PROGRAMS}</span>
        )}
        <label>Goal</label>
        <textarea className="ag-goal" value={goal} onChange={(e) => setGoal(e.target.value)} rows={3} />
        <label>Seat</label>
        <input className="ag-seat-input" value={seat} placeholder="none: a developer" onChange={(e) => setSeat(e.target.value)} />
      </div>
      <div className="ag-actions">
        <Button
          variant="primary"
          size="sm"
          icon={Play}
          disabled={!supported || busy || goal.trim() === ""}
          data-tip={supported ? "Start the program as an agent session of this host" : NO_PROGRAMS}
          onClick={() => void start()}
        >
          Start session
        </Button>
        <Button
          size="sm"
          variant="ghost"
          icon={Square}
          disabled={!stoppable}
          data-tip={stoppable ? `Stop ${stoppable.id}` : "Select a running started session to stop it"}
          onClick={() => void stop()}
        >
          Stop
        </Button>
        <Spacer />
        <Button
          size="sm"
          variant="ghost"
          icon={ClipboardCopy}
          data-tip="Copy a Claude Code command that connects to this host's MCP endpoint"
          onClick={() => {
            void navigator.clipboard?.writeText(`claude mcp add --transport http amoris ${mcpUrl()}`);
            useUi.getState().toast({ kind: "success", title: "Copied", body: `claude mcp add --transport http amoris ${mcpUrl()}` });
          }}
        >
          MCP endpoint
        </Button>
      </div>
    </div>
  );
}

export function AgentPanel() {
  const rows = useAgent((s) => s.sessions);
  const all = useAgent((s) => s.feed);
  const hidden = useAgent((s) => s.hidden);
  const clear = useAgent((s) => s.clear);
  const [session, setSession] = useState("all");
  // The host's rows, the most recently active first.
  const sessions = useMemo(() => [...rows].sort((a, b) => b.last_ts - a.last_ts), [rows]);
  const feed = useMemo(() => (hidden ? all.filter((l) => l.seq > hidden) : all), [all, hidden]);
  const shown = session === "all" ? feed : feed.filter((l) => l.session === session);
  const now = Date.now();

  const toolbar = (
    <Toolbar>
      <SelectField
        value={session}
        onChange={setSession}
        options={[
          { value: "all", label: `All sessions (${sessions.length})` },
          ...sessions.map((s) => ({ value: s.id, label: s.label ? `${s.id} · ${s.label}` : s.id })),
        ]}
      />
      <Spacer />
      <IconButton icon={X} label="Clear the feed" onClick={clear} />
    </Toolbar>
  );

  // Group consecutive lines of one session under a header.
  const groups: { session: string; lines: AgentLine[] }[] = [];
  for (const l of shown.slice(-400)) {
    const g = groups.at(-1);
    if (g && g.session === l.session) g.lines.push(l);
    else groups.push({ session: l.session, lines: [l] });
  }

  return (
    <PanelShell toolbar={toolbar} className="agent-panel">
      <NewSession selected={sessions.find((s) => s.id === session)} />
      {sessions.length > 0 && (
        <div className="ag-sessions">
          {sessions.map((s) => (
            <button
              key={s.id}
              type="button"
              className={cx("ag-session", session === s.id && "is-on", !s.live && "is-closed")}
              data-session={s.id}
              data-tip={`${describe(s)}${s.last_method ? `; last called ${s.last_method}` : ""}${s.live ? "" : "; not connected"}`}
              onClick={() => setSession(session === s.id ? "all" : s.id)}
            >
              <span className={cx("ag-live", s.live && "is-live")} />
              <span className="mono">{s.id}</span>
              {s.label && <span className="ag-label">{s.label}</span>}
              <span className="ag-tag">{s.transport}</span>
              {s.seat && <span className="ag-tag ag-seat">{s.seat}</span>}
              <span className="ag-count">
                {s.calls} {s.calls === 1 ? "call" : "calls"}
                {s.failed_calls > 0 && <span className="ag-failed"> · {s.failed_calls} failed</span>}
              </span>
              <span className="ag-ago">{ago(s.last_ts, now)}</span>
            </button>
          ))}
        </div>
      )}
      {groups.length === 0 ? (
        <EmptyState icon={Bot} title="No agent activity yet">
          When an agent connects to the host (over MCP, or with amoris mcp), its session is listed here with each call and its result, also those made before this editor opened; its edits land in History.
        </EmptyState>
      ) : (
        <div className="ag-feed">
          {groups.map((g, i) => (
            <div key={i} className="ag-group">
              <div className="ag-group-head">
                <Bot size={13} />
                <span className="mono">{g.session}</span>
              </div>
              {g.lines.map((l) => (
                <Line key={l.seq} l={l} />
              ))}
            </div>
          ))}
        </div>
      )}
    </PanelShell>
  );
}
