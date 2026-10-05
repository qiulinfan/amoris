// What agents do, live: every MCP call an agent makes reaches the editor as an `agent` event (call,
// result, message), grouped by session. Starting a session from here waits for the host's
// agent.session methods; agents connect over MCP today.

import { ArrowRight, Bot, Check, ChevronRight, ClipboardCopy, MessageSquare, Play, Square, X } from "lucide-react";
import { useMemo, useState } from "react";
import { host } from "../../host/api";
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

function Line({ l }: { l: AgentLine }) {
  const Icon = l.kind === "call" ? ArrowRight : l.kind === "message" ? MessageSquare : l.ok === false ? X : Check;
  return (
    <div className={cx("ag-line", `ag-${l.kind}`, l.ok === false && "is-failed")}>
      <span className="ag-time mono">{timeOfDay(l.ts)}</span>
      <span className="ag-icon">
        <Icon size={12} />
      </span>
      {l.method && <span className="ag-method mono">{l.method}</span>}
      <span className="ag-summary">{l.summary}</span>
    </div>
  );
}

function NewSession() {
  const catalog = useConnection((s) => s.catalog);
  const supported = catalog.some((c) => c.name === "agent.session.start");
  const [model, setModel] = useState("claude-opus-5.5");
  const [goal, setGoal] = useState("Make every crate reachable on the sloop's course, then sail the course and report the tally.");
  const [edit, setEdit] = useState(true);
  const [open, setOpen] = useState(() => localStorage.getItem("amoris.editor.agent.new") === "1");
  const toggle = () => {
    localStorage.setItem("amoris.editor.agent.new", open ? "0" : "1");
    setOpen(!open);
  };
  if (!open) {
    return (
      <button type="button" className="ag-new ag-new-collapsed" onClick={toggle}>
        <Bot size={15} />
        <span>New agent session…</span>
        {!supported && <span className="ag-soon">preview</span>}
        <ChevronRight size={14} className="ag-chev" />
      </button>
    );
  }
  return (
    <div className="ag-new">
      <div className="ag-new-head" onClick={toggle}>
        <Bot size={15} />
        <span>New agent session</span>
        {!supported && <span className="ag-soon" data-tip="The host protocol has no agent.session.* methods yet">preview</span>}
        <ChevronRight size={14} className="ag-chev is-open" />
      </div>
      <div className="ag-form">
        <label>Model</label>
        <SelectField
          value={model}
          onChange={setModel}
          options={[
            { value: "claude-opus-5.5", label: "Claude Opus 5.5" },
            { value: "claude-sonnet-5", label: "Claude Sonnet 5" },
            { value: "glm-5.3-flash", label: "GLM 5.3 Flash (opencode)" },
          ]}
        />
        <label>Goal</label>
        <textarea className="ag-goal" value={goal} onChange={(e) => setGoal(e.target.value)} rows={3} />
        <label>Tools</label>
        <div className="ag-tools">
          <span className="ag-tool is-on">world (read)</span>
          <span className={cx("ag-tool", edit && "is-on")} onClick={() => setEdit(!edit)}>
            world.edit
          </span>
          <span className="ag-tool is-on">events</span>
          <span className="ag-tool is-on">debug</span>
          <span className="ag-tool">player seat</span>
        </div>
      </div>
      <div className="ag-actions">
        <Button
          variant="primary"
          size="sm"
          icon={Play}
          disabled={!supported}
          data-tip={supported ? "Start" : "Waiting for agent.session.start on the host; connect an agent over MCP meanwhile"}
          onClick={() => useUi.getState().toast({ kind: "info", title: "Agent sessions", body: "Starting sessions from the editor needs agent.session.start on the host." })}
        >
          Start session
        </Button>
        <Button size="sm" variant="ghost" icon={Square} disabled>
          Stop
        </Button>
        <Spacer />
        <Button
          size="sm"
          variant="ghost"
          icon={ClipboardCopy}
          data-tip="Copy a Claude Code command that connects to this host's MCP endpoint"
          onClick={() => {
            void navigator.clipboard?.writeText(`claude mcp add --transport http pocket ${mcpUrl()}`);
            useUi.getState().toast({ kind: "success", title: "Copied", body: `claude mcp add --transport http pocket ${mcpUrl()}` });
          }}
        >
          MCP endpoint
        </Button>
      </div>
    </div>
  );
}

export function AgentPanel() {
  const feed = useAgent((s) => s.feed);
  const clear = useAgent((s) => s.clear);
  const [session, setSession] = useState("all");
  const sessions = useMemo(() => {
    const m = new Map<string, { calls: number; last: number }>();
    for (const l of feed) {
      const s = m.get(l.session) ?? { calls: 0, last: 0 };
      if (l.kind === "call") s.calls++;
      s.last = Math.max(s.last, l.ts);
      m.set(l.session, s);
    }
    return [...m.entries()].sort((a, b) => b[1].last - a[1].last);
  }, [feed]);
  const shown = session === "all" ? feed : feed.filter((l) => l.session === session);
  const now = Date.now();

  const toolbar = (
    <Toolbar>
      <SelectField
        value={session}
        onChange={setSession}
        options={[{ value: "all", label: `All sessions (${sessions.length})` }, ...sessions.map(([s]) => ({ value: s, label: s }))]}
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
      <NewSession />
      {sessions.length > 0 && (
        <div className="ag-sessions">
          {sessions.map(([s, info]) => (
            <button key={s} type="button" className={cx("ag-session", session === s && "is-on")} onClick={() => setSession(session === s ? "all" : s)}>
              <span className={cx("ag-live", now - info.last < 15000 && "is-live")} />
              <span className="mono">{s}</span>
              <span className="ag-count">{info.calls} calls</span>
              <span className="ag-ago">{ago(info.last, now)}</span>
            </button>
          ))}
        </div>
      )}
      {groups.length === 0 ? (
        <EmptyState icon={Bot} title="No agent activity yet">
          When an agent calls the host over MCP, each call and its result appear here, and its edits land in History.
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
                <Line key={l.id} l={l} />
              ))}
            </div>
          ))}
        </div>
      )}
    </PanelShell>
  );
}
