// Game events as a table (tick, name, subject, data) with filtering; selecting one asks the host
// why it happened (`events.why`) and shows the cause chain, oldest first.

import { ArrowDownToLine, Ban, GitBranch, Zap } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { api } from "../../host/api";
import type { GameEvent } from "../../host/protocol";
import { reportError } from "../../actions/report";
import { useEvents } from "../../state/events";
import { useSelection } from "../../state/selection";
import { entityName } from "../../state/world";
import { IconButton } from "../../ui/Button";
import { cx } from "../../ui/cx";
import { compactJson, hue } from "../../ui/format";
import { EmptyState, PanelShell } from "../../ui/Panel";
import { SearchInput } from "../../ui/SearchInput";
import { SplitPane } from "../../ui/SplitPane";
import { Toolbar, ToolbarSeparator } from "../../ui/Toolbar";
import "./events.css";

function Subject({ id }: { id: number | null }) {
  if (id === null) return <span className="muted">—</span>;
  return (
    <button
      type="button"
      className="link"
      onClick={(e) => {
        e.stopPropagation();
        useSelection.getState().set([id]);
      }}
    >
      {entityName(id)}
    </button>
  );
}

function Why({ chain, selected }: { chain: GameEvent[] | null; selected: GameEvent | undefined }) {
  if (!selected) return <EmptyState icon={GitBranch} title="Select an event">Its cause chain (events.why) appears here.</EmptyState>;
  return (
    <div className="why">
      <div className="why-head">
        <span className="ev-dot" style={{ background: `hsl(${hue(selected.name)} 70% 60%)` }} />
        <span className="why-name">{selected.name}</span>
        <span className="mono muted">
          #{selected.seq} · tick {selected.tick}
        </span>
      </div>
      <div className="why-title">Why (cause chain)</div>
      {!chain ? (
        <div className="muted pad">Asking the host…</div>
      ) : chain.length <= 1 ? (
        <div className="muted pad">No recorded cause: this event started its chain.</div>
      ) : (
        <ol className="chain">
          {chain.map((e, i) => (
            <li key={e.seq} className={cx(e.seq === selected.seq && "is-self")} onClick={() => void select(e)}>
              <span className="chain-step">{i + 1}</span>
              <span className="ev-dot" style={{ background: `hsl(${hue(e.name)} 70% 60%)` }} />
              <span className="chain-name">{e.name}</span>
              <span className="mono muted">tick {e.tick}</span>
              <span className="chain-subject">
                <Subject id={e.subject} />
              </span>
              <span className="chain-data mono">{compactJson(e.data, 60)}</span>
            </li>
          ))}
        </ol>
      )}
      <div className="why-title">Data</div>
      <pre className="why-data">{JSON.stringify(selected.data, null, 2)}</pre>
    </div>
  );
}

async function select(e: GameEvent) {
  useEvents.getState().select(e.seq, null);
  try {
    const chain = await api.events.why(e.seq);
    if (useEvents.getState().selected === e.seq) useEvents.getState().select(e.seq, chain);
  } catch (err) {
    reportError(err, "events.why");
    useEvents.getState().select(e.seq, [e]);
  }
}

export function EventsPanel() {
  const { events, selected, why, clear } = useEvents();
  const [q, setQ] = useState("");
  const [follow, setFollow] = useState(true);
  const body = useRef<HTMLDivElement>(null);
  const rows = useMemo(() => {
    const needle = q.toLowerCase();
    const out = !needle
      ? events
      : events.filter((e) => e.name.toLowerCase().includes(needle) || (e.subject !== null && entityName(e.subject).toLowerCase().includes(needle)) || compactJson(e.data, 400).toLowerCase().includes(needle));
    return out.slice(-1000);
  }, [events, q]);
  const sel = events.find((e) => e.seq === selected);

  useEffect(() => {
    if (follow && body.current) body.current.scrollTop = body.current.scrollHeight;
  }, [rows, follow]);

  const toolbar = (
    <Toolbar>
      <SearchInput value={q} onChange={setQ} placeholder="Filter by name, subject or data" />
      <span className="toolbar-note">{events.length} events</span>
      <ToolbarSeparator />
      <IconButton icon={ArrowDownToLine} label="Follow new events" active={follow} onClick={() => setFollow(!follow)} />
      <IconButton icon={Ban} label="Clear the list" onClick={clear} />
    </Toolbar>
  );

  return (
    <PanelShell toolbar={toolbar} scroll={false} className="events-panel">
      <SplitPane id="events" initial={560} min={260}>
        <div className="ev-table">
          <div className="ev-row ev-header">
            <span>seq</span>
            <span>tick</span>
            <span>event</span>
            <span>subject</span>
            <span>data</span>
          </div>
          <div
            ref={body}
            className="ev-body"
            onScroll={(e) => {
              const el = e.currentTarget;
              const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
              if (atBottom !== follow) setFollow(atBottom);
            }}
          >
            {rows.length === 0 && <EmptyState icon={Zap} title="No events yet">Press Play; game events stream here as systems emit them.</EmptyState>}
            {rows.map((e) => (
              <div key={e.seq} className={cx("ev-row", e.seq === selected && "is-selected")} onClick={() => void select(e)}>
                <span className="mono muted">{e.seq}</span>
                <span className="mono">{e.tick}</span>
                <span className="ev-name">
                  <span className="ev-dot" style={{ background: `hsl(${hue(e.name)} 70% 60%)` }} />
                  {e.name}
                  {e.cause !== null && <GitBranch size={11} className="caused" data-tip={`caused by #${e.cause}`} />}
                </span>
                <span>
                  <Subject id={e.subject} />
                </span>
                <span className="mono ev-data">{compactJson(e.data)}</span>
              </div>
            ))}
          </div>
        </div>
        <Why chain={why} selected={sel} />
      </SplitPane>
    </PanelShell>
  );
}
