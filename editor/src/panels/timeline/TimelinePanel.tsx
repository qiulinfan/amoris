// The timeline: kept snapshots (`snapshots.list`) and events on a tick ruler with the playhead.
// Drag across it to scrub: on release the host rewinds there (`debug.rewind`: the snapshot at or
// before the tick, replayed to it). Click a snapshot's diamond to restore it exactly.

import { CirclePause, CirclePlay, ClockArrowDown, SkipBack, StepForward } from "lucide-react";
import { useMemo, useState } from "react";
import { restoreSnapshot, rewind, setPaused, stepTick } from "../../actions/time";
import { useEvents } from "../../state/events";
import { useSelection } from "../../state/selection";
import { useSession } from "../../state/session";
import { IconButton } from "../../ui/Button";
import { clock, hue } from "../../ui/format";
import { EmptyState, PanelShell } from "../../ui/Panel";
import { Spacer, Toolbar, ToolbarSeparator } from "../../ui/Toolbar";
import { useSize } from "../../ui/useSize";
import "./timeline.css";

const LANE_H = 22;
const RULER_H = 24;
const LEFT = 120;

function niceStep(range: number, px: number): number {
  const target = (range / Math.max(1, px)) * 90;
  const pow = Math.pow(10, Math.floor(Math.log10(Math.max(1, target))));
  for (const m of [1, 2, 5, 10]) if (pow * m >= target) return pow * m;
  return pow * 10;
}

export function TimelinePanel() {
  const status = useSession((s) => s.status);
  const snapshots = useSession((s) => s.snapshots);
  const events = useEvents((s) => s.events);
  const selection = useSelection((s) => s.ids);
  const [wrap, size] = useSize<HTMLDivElement>();
  const width = size.width || 600;
  const [scrub, setScrub] = useState<number | null>(null);
  const [hover, setHover] = useState<number | null>(null);

  const tick = status?.tick ?? 0;
  const lo = snapshots.length ? Math.min(snapshots[0]!.tick, tick) : 0;
  const hi = Math.max(tick, snapshots.at(-1)?.tick ?? 0, lo + 60);
  const span = Math.max(1, hi - lo);
  const trackW = Math.max(10, width - LEFT - 12);
  const x = (t: number) => LEFT + ((t - lo) / span) * trackW;
  const tickAt = (px: number) => Math.round(lo + Math.max(0, Math.min(1, (px - LEFT) / trackW)) * span);

  const lanes = useMemo(() => {
    const inRange = events.filter((e) => e.tick >= lo && e.tick <= hi);
    const counts = new Map<string, number>();
    for (const e of inRange) counts.set(e.name, (counts.get(e.name) ?? 0) + 1);
    const names = [...counts.entries()].sort((a, b) => b[1] - a[1]).slice(0, 6).map(([n]) => n);
    const sel = new Set(selection);
    return {
      byName: names.map((n) => ({ name: n, ticks: inRange.filter((e) => e.name === n) })),
      selected: inRange.filter((e) => e.subject !== null && sel.has(e.subject)),
    };
  }, [events, lo, hi, selection]);

  const playing = status?.mode === "play";
  const step = niceStep(span, trackW);
  const firstLabel = Math.ceil(lo / step) * step;
  const labels: number[] = [];
  for (let t = firstLabel; t <= hi; t += step) labels.push(t);
  const height = RULER_H + LANE_H * (2 + lanes.byName.length) + 8;

  const toolbar = (
    <Toolbar>
      <IconButton
        icon={SkipBack}
        label="Back to the previous snapshot"
        disabled={!playing || snapshots.length === 0}
        onClick={() => {
          const prev = [...snapshots].reverse().find((s) => s.tick < tick);
          if (prev) void restoreSnapshot(prev.tick);
        }}
      />
      {status?.paused || !playing ? (
        <IconButton icon={CirclePlay} label="Resume" tone="play" disabled={!playing} onClick={() => void setPaused(false)} />
      ) : (
        <IconButton icon={CirclePause} label="Pause" onClick={() => void setPaused(true)} />
      )}
      <IconButton icon={StepForward} label="Step one tick" onClick={() => void stepTick()} />
      <ToolbarSeparator />
      <span className="tl-readout mono">
        tick <b>{tick}</b> · {clock(status?.t_s ?? 0)}
      </span>
      <Spacer />
      <span className="toolbar-note">
        {snapshots.length} snapshots kept{snapshots.length ? ` (${snapshots[0]!.tick}–${snapshots.at(-1)!.tick})` : ""}
      </span>
    </Toolbar>
  );

  if (!playing && snapshots.length === 0) {
    return (
      <PanelShell toolbar={toolbar}>
        <EmptyState icon={ClockArrowDown} title="Nothing recorded yet">
          Press Play. The host keeps a ring of snapshots; scrub here to rewind to any tick and replay.
        </EmptyState>
      </PanelShell>
    );
  }

  const shownTick = scrub ?? hover;

  return (
    <PanelShell toolbar={toolbar} className="timeline">
      <div ref={wrap} className="tl-wrap">
        <svg
          width={width}
          height={height}
          className="tl-svg"
          onPointerDown={(e) => {
            const r = e.currentTarget.getBoundingClientRect();
            if (e.clientX - r.left < LEFT) return;
            e.currentTarget.setPointerCapture(e.pointerId);
            setScrub(tickAt(e.clientX - r.left));
          }}
          onPointerMove={(e) => {
            const r = e.currentTarget.getBoundingClientRect();
            const px = e.clientX - r.left;
            if (scrub !== null) setScrub(tickAt(px));
            else setHover(px >= LEFT ? tickAt(px) : null);
          }}
          onPointerLeave={() => setHover(null)}
          onPointerUp={() => {
            if (scrub !== null && scrub !== tick) void rewind(scrub);
            setScrub(null);
          }}
        >
          <rect x={LEFT} y={0} width={trackW} height={RULER_H} className="tl-ruler" />
          {labels.map((t) => (
            <g key={t}>
              <line x1={x(t)} x2={x(t)} y1={RULER_H - 6} y2={height} className="tl-grid" />
              <text x={x(t) + 3} y={14} className="tl-label">
                {t}
              </text>
            </g>
          ))}
          <text x={8} y={RULER_H + 15} className="tl-lane">
            Snapshots
          </text>
          {snapshots.map((s) => (
            <rect
              key={s.tick}
              x={x(s.tick) - 4}
              y={RULER_H + 7}
              width={8}
              height={8}
              transform={`rotate(45 ${x(s.tick)} ${RULER_H + 11})`}
              className="tl-snap"
              onPointerDown={(e) => {
                e.stopPropagation();
                void restoreSnapshot(s.tick);
              }}
            >
              <title>{`Snapshot at tick ${s.tick}${s.hash ? ` · ${s.hash.slice(0, 8)}` : ""} — click to restore`}</title>
            </rect>
          ))}
          <text x={8} y={RULER_H + LANE_H + 15} className="tl-lane">
            Selection
          </text>
          {lanes.selected.map((e) => (
            <circle key={e.seq} cx={x(e.tick)} cy={RULER_H + LANE_H + 11} r={3.5} fill={`hsl(${hue(e.name)} 70% 60%)`}>
              <title>{`${e.name} · tick ${e.tick}`}</title>
            </circle>
          ))}
          {lanes.byName.map((lane, i) => {
            const y = RULER_H + LANE_H * (2 + i);
            return (
              <g key={lane.name}>
                <text x={8} y={y + 15} className="tl-lane">
                  {lane.name.length > 15 ? `${lane.name.slice(0, 14)}…` : lane.name}
                </text>
                {lane.ticks.map((e) => (
                  <rect key={e.seq} x={x(e.tick) - 1.5} y={y + 5} width={3} height={12} rx={1} fill={`hsl(${hue(e.name)} 70% 60%)`}>
                    <title>{`${e.name} #${e.seq} · tick ${e.tick}`}</title>
                  </rect>
                ))}
              </g>
            );
          })}
          <line x1={x(tick)} x2={x(tick)} y1={0} y2={height} className="tl-playhead" />
          <path d={`M ${x(tick) - 5} 0 L ${x(tick) + 5} 0 L ${x(tick)} 7 Z`} className="tl-playhead-cap" />
          {shownTick !== null && (
            <g>
              <line x1={x(shownTick)} x2={x(shownTick)} y1={0} y2={height} className={scrub !== null ? "tl-scrub" : "tl-hover"} />
              <rect x={x(shownTick) + 4} y={2} width={64} height={17} rx={4} className="tl-tip" />
              <text x={x(shownTick) + 9} y={14} className="tl-tip-text">
                {scrub !== null ? "→ " : ""}
                {shownTick}
              </text>
            </g>
          )}
        </svg>
      </div>
    </PanelShell>
  );
}
