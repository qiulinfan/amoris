// The profiler: per sample (4 a second), the host's CPU time per tick stacked by system and the
// viewport's frame time against the 60 Hz budget, then averages over a short window: the game
// thread's systems (the host's `profile` pushes), the tick's phases, and the viewport's GPU passes
// (its own timestamps: the editor draws the frame, the host does not).

import { Activity, Ban, Pause, Play } from "lucide-react";
import { useMemo } from "react";
import { useProfile, type ProfileSample } from "../../state/profile";
import { useConnection } from "../../state/connection";
import { IconButton } from "../../ui/Button";
import { hue, ms } from "../../ui/format";
import { EmptyState, PanelShell } from "../../ui/Panel";
import { useSize } from "../../ui/useSize";
import { ProfileChart } from "./ProfileChart";
import { Spacer, Toolbar, ToolbarSeparator } from "../../ui/Toolbar";
import "./profiler.css";

const WINDOW = 12; // samples (3 s at 4 Hz)

interface Row {
  name: string;
  avg: number;
  max: number;
}

function Bars({ title, tip, rows, total, empty }: { title: string; tip?: string; rows: Row[]; total: number | null; empty?: string }) {
  const top = Math.max(0.001, ...rows.map((r) => r.max));
  return (
    <div className="prof-col">
      <div className="prof-col-head" data-tip={tip}>
        <span>{title}</span>
        <span className="mono">{total != null ? ms(total) : "–"}</span>
      </div>
      {rows.length === 0 && empty && <div className="prof-empty">{empty}</div>}
      {rows.map((r) => (
        <div key={r.name} className="prof-row" data-tip={`${r.name}: avg ${ms(r.avg)}, max ${ms(r.max)} over the last ${WINDOW} samples`}>
          <span className="prof-name">{r.name}</span>
          <div className="prof-bar">
            <div className="prof-max" style={{ width: `${(r.max / top) * 100}%` }} />
            <div className="prof-fill" style={{ width: `${(r.avg / top) * 100}%`, background: `hsl(${hue(r.name)} 55% 58%)` }} />
          </div>
          <span className="prof-val mono">{ms(r.avg)}</span>
        </div>
      ))}
    </div>
  );
}

/** Per name, the average over the samples that measured something (host rows: weighted by their
 * ticks, so a quarter second without ticks counts for nothing) and the largest value seen. */
function hostRows(samples: ProfileSample[], key: "systems" | "phases"): Row[] {
  const acc = new Map<string, { sum: number; max: number }>();
  let ticks = 0;
  for (const s of samples) {
    if (!s.ticks) continue;
    ticks += s.ticks;
    for (const r of s[key]) {
      const a = acc.get(r.name) ?? { sum: 0, max: 0 };
      a.sum += r.ms * s.ticks;
      a.max = Math.max(a.max, r.max_ms ?? r.ms);
      acc.set(r.name, a);
    }
  }
  return [...acc.entries()].map(([name, a]) => ({ name, avg: a.sum / Math.max(1, ticks), max: a.max }));
}

function gpuRows(samples: ProfileSample[]): Row[] {
  const acc = new Map<string, { sum: number; max: number; n: number }>();
  for (const s of samples) {
    for (const p of s.gpu) {
      const a = acc.get(p.pass) ?? { sum: 0, max: 0, n: 0 };
      a.sum += p.ms;
      a.max = Math.max(a.max, p.ms);
      a.n++;
      acc.set(p.pass, a);
    }
  }
  return [...acc.entries()].map(([name, a]) => ({ name, avg: a.sum / Math.max(1, a.n), max: a.max }));
}

/** The average of a field over the samples that have it. */
function mean(samples: ProfileSample[], f: (s: ProfileSample) => number | null | undefined): number | null {
  let sum = 0;
  let n = 0;
  for (const s of samples) {
    const v = f(s);
    if (v == null) continue;
    sum += v;
    n++;
  }
  return n ? sum / n : null;
}

export function ProfilerPanel() {
  const { samples, frozen, setFrozen, clear } = useProfile();
  const viewport = useConnection((s) => s.viewport);
  const [wrap, size] = useSize<HTMLDivElement>();
  const recent = useMemo(() => samples.slice(-WINDOW), [samples]);
  const cpu = useMemo(() => hostRows(recent, "systems"), [recent]);
  const phases = useMemo(() => hostRows(recent, "phases"), [recent]);
  const gpu = useMemo(() => gpuRows(recent).sort((a, b) => b.avg - a.avg), [recent]);
  const last = samples.at(-1);
  const ticked = recent.some((s) => s.ticks > 0);
  const weighted = (f: (s: ProfileSample) => number) => {
    const n = recent.reduce((a, s) => a + s.ticks, 0);
    return n ? recent.reduce((a, s) => a + f(s) * s.ticks, 0) / n : null;
  };
  const tickMs = weighted((s) => s.tick_ms);
  const busyMs = weighted((s) => s.busy_ms);
  const frameMs = mean(recent, (s) => s.frame_ms);
  const gpuMs = mean(recent, (s) => s.gpu_ms);
  const held = recent.reduce((a, s) => a + (s.held ?? 0), 0);
  const gpuEmpty =
    viewport === "fallback"
      ? "The fallback view draws on the CPU: no GPU passes."
      : last?.frame_ms == null
        ? "No viewport is drawing."
        : "No GPU timestamps from this adapter (timestamp queries).";

  const toolbar = (
    <Toolbar>
      <IconButton icon={frozen ? Play : Pause} label={frozen ? "Resume capture" : "Freeze capture"} active={frozen} onClick={() => setFrozen(!frozen)} />
      <IconButton icon={Ban} label="Clear" onClick={clear} />
      <ToolbarSeparator />
      <span className="toolbar-note mono" data-tip="Host: the tick (Game::step) and the game thread's whole time per tick, tick-weighted over the last 3 s">
        tick {tickMs != null ? ms(tickMs) : "–"} · thread {busyMs != null ? ms(busyMs) : "–"}
      </span>
      <ToolbarSeparator />
      <span className="toolbar-note mono" data-tip="This editor's viewport: frame interval and GPU time, over the last 3 s">
        frame {frameMs != null ? ms(frameMs) : "–"} · {frameMs ? (1000 / frameMs).toFixed(0) : "–"} fps · GPU {gpuMs != null ? ms(gpuMs) : "–"}
      </span>
      {held > 0 && <span className="toolbar-note">· {held} held tick{held === 1 ? "" : "s"} left out</span>}
      <Spacer />
      {last && <span className="toolbar-note mono">tick {last.tick}</span>}
    </Toolbar>
  );

  return (
    <PanelShell toolbar={toolbar} className="profiler">
      <div ref={wrap} className="prof-graph">
        {samples.length === 0 ? (
          <EmptyState icon={Activity} title="Waiting for profile samples">The host sends its CPU timings four times a second.</EmptyState>
        ) : (
          <>
            <div className="prof-graph-label">
              <span>Frame</span>
              <span className="mono">{last?.frame_ms != null ? ms(last.frame_ms) : "–"}</span>
              <span className="budget-label">last {Math.round(samples.length / 4)} s · 4 samples/s</span>
            </div>
            <ProfileChart samples={samples} width={Math.max(100, size.width - 24)} height={92} />
          </>
        )}
      </div>
      {samples.length > 0 && (
        <div className="prof-cols">
          <Bars
            title="CPU · game thread, per tick"
            tip="Each system's own time (a script system's out of script.update), then tick.other (the step outside its systems), loop.present (the render feed) and loop.publish (snapshot and streams)"
            rows={cpu}
            total={busyMs}
            empty={ticked ? undefined : "No ticks in the last 3 s: press Play or step."}
          />
          <Bars title="CPU · phases" rows={phases} total={tickMs} empty={ticked ? undefined : "–"} />
          <Bars title="GPU · viewport passes" rows={gpu} total={gpuMs} empty={gpuEmpty} />
        </div>
      )}
    </PanelShell>
  );
}
