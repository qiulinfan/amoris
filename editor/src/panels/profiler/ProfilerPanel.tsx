// The profiler: frame time over the last minute (sparkline against the 60 Hz budget), and per-system
// CPU and per-pass GPU milliseconds from `profile` events, averaged over a short window.

import { Activity, Ban, Pause, Play } from "lucide-react";
import { useMemo } from "react";
import { useProfile } from "../../state/profile";
import { IconButton } from "../../ui/Button";
import { hue, ms } from "../../ui/format";
import { EmptyState, PanelShell } from "../../ui/Panel";
import { useSize } from "../../ui/useSize";
import { ProfileChart } from "./ProfileChart";
import { Spacer, Toolbar, ToolbarSeparator } from "../../ui/Toolbar";
import "./profiler.css";

const WINDOW = 12; // samples (3 s at 4 Hz)

function Bars({ title, rows, total }: { title: string; rows: { name: string; avg: number; max: number }[]; total: number }) {
  const top = Math.max(0.001, ...rows.map((r) => r.max));
  return (
    <div className="prof-col">
      <div className="prof-col-head">
        <span>{title}</span>
        <span className="mono">{ms(total)}</span>
      </div>
      {rows.map((r) => (
        <div key={r.name} className="prof-row" data-tip={`${r.name}: avg ${ms(r.avg)}, max ${ms(r.max)} over ${WINDOW} samples`}>
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

function average<K extends "systems" | "gpu">(samples: ReturnType<typeof useProfile.getState>["samples"], key: K) {
  const acc = new Map<string, { sum: number; max: number; n: number }>();
  for (const s of samples) {
    for (const item of s[key] as { name?: string; pass?: string; ms: number }[]) {
      const name = item.name ?? item.pass ?? "?";
      const a = acc.get(name) ?? { sum: 0, max: 0, n: 0 };
      a.sum += item.ms;
      a.max = Math.max(a.max, item.ms);
      a.n++;
      acc.set(name, a);
    }
  }
  return [...acc.entries()].map(([name, a]) => ({ name, avg: a.sum / Math.max(1, a.n), max: a.max })).sort((a, b) => b.avg - a.avg);
}

export function ProfilerPanel() {
  const { samples, frozen, setFrozen, clear } = useProfile();
  const [wrap, size] = useSize<HTMLDivElement>();
  const recent = samples.slice(-WINDOW);
  const cpu = useMemo(() => average(recent, "systems"), [recent]);
  const gpu = useMemo(() => average(recent, "gpu"), [recent]);
  const last = samples.at(-1);
  const avgFrame = recent.reduce((a, s) => a + s.frame_ms, 0) / Math.max(1, recent.length);
  const cpuTotal = cpu.reduce((a, r) => a + r.avg, 0);
  const gpuTotal = gpu.reduce((a, r) => a + r.avg, 0);

  const toolbar = (
    <Toolbar>
      <IconButton icon={frozen ? Play : Pause} label={frozen ? "Resume capture" : "Freeze capture"} active={frozen} onClick={() => setFrozen(!frozen)} />
      <IconButton icon={Ban} label="Clear" onClick={clear} />
      <ToolbarSeparator />
      <span className="toolbar-note mono">
        frame {ms(avgFrame)} · {avgFrame > 0 ? (1000 / avgFrame).toFixed(0) : "–"} fps · CPU {ms(cpuTotal)} · GPU {ms(gpuTotal)}
      </span>
      <Spacer />
      {last && <span className="toolbar-note mono">tick {last.tick}</span>}
    </Toolbar>
  );

  return (
    <PanelShell toolbar={toolbar} className="profiler">
      <div ref={wrap} className="prof-graph">
        {samples.length === 0 ? (
          <EmptyState icon={Activity} title="Waiting for profile samples">The host sends CPU and GPU timings four times a second.</EmptyState>
        ) : (
          <>
            <div className="prof-graph-label">
              <span>Frame</span>
              <span className="mono">{last ? ms(last.frame_ms) : ""}</span>
              <span className="budget-label">last {Math.round(samples.length / 4)} s · 4 samples/s</span>
            </div>
            <ProfileChart samples={samples} width={Math.max(100, size.width - 24)} height={92} />
          </>
        )}
      </div>
      {samples.length > 0 && (
        <div className="prof-cols">
          <Bars title="CPU · systems" rows={cpu} total={cpuTotal} />
          <Bars title="GPU · passes" rows={gpu} total={gpuTotal} />
        </div>
      )}
    </PanelShell>
  );
}
