// The axis gizmo (as in Blender and Godot): the world axes as the camera sees them; click an axis to
// look along it, the centre to switch perspective and orthographic.

import { useSyncExternalStore } from "react";
import { cameraOf, View } from "../../viewport/camera";
import { dot, type Vec3 } from "../../viewport/math";
import type { ViewportController } from "./controller";

const AXES: { dir: Vec3; label: string; look: "+x" | "-x" | "+y" | "-y" | "+z" | "-z"; color: string; positive: boolean }[] = [
  { dir: [1, 0, 0], label: "X", look: "+x", color: "var(--axis-x)", positive: true },
  { dir: [0, 1, 0], label: "Y", look: "+y", color: "var(--axis-y)", positive: true },
  { dir: [0, 0, 1], label: "Z", look: "+z", color: "var(--axis-z)", positive: true },
  { dir: [-1, 0, 0], label: "", look: "-x", color: "var(--axis-x)", positive: false },
  { dir: [0, -1, 0], label: "", look: "-y", color: "var(--axis-y)", positive: false },
  { dir: [0, 0, -1], label: "", look: "-z", color: "var(--axis-z)", positive: false },
];

export function AxisGizmo({ ctl }: { ctl: ViewportController }) {
  useSyncExternalStore(
    (fn) => ctl.subscribe(fn),
    () => `${ctl.orbit.yaw.toFixed(4)}|${ctl.orbit.pitch.toFixed(4)}|${ctl.orbit.ortho}`,
  );
  const view = new View(cameraOf(ctl.orbit), 100, 100);
  const size = 84;
  const c = size / 2;
  const R = 28;
  const items = AXES.map((a) => ({
    ...a,
    x: c + dot(a.dir, view.right) * R,
    y: c - dot(a.dir, view.up) * R,
    depth: dot(a.dir, view.forward),
  })).sort((a, b) => b.depth - a.depth);
  return (
    <div className="axis-gizmo" onPointerDown={(e) => e.stopPropagation()}>
      <svg width={size} height={size} viewBox={`0 0 ${size} ${size}`}>
        <circle cx={c} cy={c} r={c - 2} className="axis-bg" onClick={() => ctl.look("persp")}>
          <title>{ctl.orbit.ortho ? "Orthographic: click for perspective" : "Perspective: click for orthographic"}</title>
        </circle>
        {items.map((a) => (
          <g key={a.look} className="axis-item" onClick={() => ctl.look(a.look)}>
            <title>{`Look along ${a.look}`}</title>
            {a.positive && <line x1={c} y1={c} x2={a.x} y2={a.y} stroke={a.color} strokeWidth={2} />}
            <circle cx={a.x} cy={a.y} r={a.positive ? 8 : 6} fill={a.positive ? a.color : "var(--bg-1)"} stroke={a.color} strokeWidth={a.positive ? 0 : 1.5} opacity={a.depth > 0.3 && !a.positive ? 0.6 : 1} />
            {a.label && (
              <text x={a.x} y={a.y + 0.5} className="axis-label">
                {a.label}
              </text>
            )}
          </g>
        ))}
      </svg>
    </div>
  );
}
