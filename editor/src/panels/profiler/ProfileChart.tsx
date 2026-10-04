// The profiler's timeline chart: per sample, the tick's CPU time stacked by system (bars, left
// scale) and the frame time (line, right scale, 0–33 ms with the 60 Hz budget marked). Hover for
// the numbers of one sample.

import { useState } from "react";
import type { ProfileSample } from "../../host/protocol";
import { hue, ms } from "../../ui/format";

const FRAME_MAX = 1000 / 30;

export function ProfileChart({ samples, width, height }: { samples: ProfileSample[]; width: number; height: number }) {
  const [hover, setHover] = useState<number | null>(null);
  const n = Math.max(60, samples.length);
  const bw = width / n;
  const totals = samples.map((s) => s.systems.reduce((a, x) => a + x.ms, 0));
  const cpuMax = Math.max(0.05, ...totals) * 1.25;
  const yCpu = (v: number) => height - (v / cpuMax) * height;
  const yFrame = (v: number) => height - (Math.min(v, FRAME_MAX) / FRAME_MAX) * height;
  const x0 = width - samples.length * bw;
  const line = samples.map((s, i) => `${(x0 + (i + 0.5) * bw).toFixed(1)},${yFrame(s.frame_ms).toFixed(1)}`).join(" ");
  const h = hover !== null ? samples[hover] : undefined;
  return (
    <div className="prof-chart">
      <svg
        width={width}
        height={height}
        onPointerMove={(e) => {
          const r = e.currentTarget.getBoundingClientRect();
          const i = Math.floor((e.clientX - r.left - x0) / bw);
          setHover(i >= 0 && i < samples.length ? i : null);
        }}
        onPointerLeave={() => setHover(null)}
      >
        <line x1={0} x2={width} y1={yFrame(1000 / 60)} y2={yFrame(1000 / 60)} className="prof-budget" />
        <text x={4} y={yFrame(1000 / 60) - 4} className="prof-axis">
          16.7 ms
        </text>
        {samples.map((s, i) => {
          let acc = 0;
          return (
            <g key={i} opacity={hover === null || hover === i ? 1 : 0.55}>
              {s.systems
                .filter((x) => x.ms > 0)
                .map((x) => {
                  const y1 = yCpu(acc);
                  acc += x.ms;
                  const y2 = yCpu(acc);
                  return <rect key={x.name} x={x0 + i * bw + 0.5} y={y2} width={Math.max(1, bw - 1)} height={Math.max(0, y1 - y2)} fill={`hsl(${hue(x.name)} 55% 58%)`} />;
                })}
            </g>
          );
        })}
        {samples.length > 1 && <polyline points={line} className="prof-frame-line" />}
        {h && <line x1={x0 + (hover! + 0.5) * bw} x2={x0 + (hover! + 0.5) * bw} y1={0} y2={height} className="prof-hover" />}
      </svg>
      <div className="prof-legend">
        <span>
          <i className="sw sw-bars" /> CPU per tick, by system (max {ms(cpuMax / 1.25)})
        </span>
        <span>
          <i className="sw sw-line" /> frame time
        </span>
      </div>
      {h && (
        <div className="prof-tip" style={{ left: Math.min(width - 210, Math.max(0, x0 + hover! * bw + 12)) }}>
          <div className="mono">tick {h.tick}</div>
          <div>
            frame <b className="mono">{ms(h.frame_ms)}</b> · CPU <b className="mono">{ms(totals[hover!] ?? 0)}</b>
          </div>
          {[...h.systems]
            .sort((a, b) => b.ms - a.ms)
            .slice(0, 4)
            .map((x) => (
              <div key={x.name} className="prof-tip-row">
                <i className="sw" style={{ background: `hsl(${hue(x.name)} 55% 58%)` }} />
                <span>{x.name}</span>
                <span className="mono">{ms(x.ms)}</span>
              </div>
            ))}
        </div>
      )}
    </div>
  );
}
