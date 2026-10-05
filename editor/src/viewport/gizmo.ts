// The transform gizmo: its geometry (3D arrows with cone heads, plane handles, rotation rings, scale
// cubes), sized to a constant number of pixels and built in world space so any renderer can draw it;
// hit testing on the same geometry; and the drag math that turns pointer motion into a new
// position, rotation or scale, with snapping.

import type { View } from "./camera";
import type { Gizmo, GizmoHandle, GizmoMode, GizmoShape, Rgba } from "./engine";
import {
  add,
  closestOnLine,
  cross,
  DEG,
  dot,
  length,
  normalize,
  qaxis,
  qmul,
  qnormalize,
  rayPlane,
  rotate,
  scale,
  snapTo,
  sub,
  type Quat,
  type Vec3,
} from "./math";

export const AXIS_COLORS: Rgba[] = [
  [0.94, 0.33, 0.31, 1],
  [0.55, 0.8, 0.29, 1],
  [0.26, 0.6, 0.96, 1],
];
const HIGHLIGHT: Rgba = [1, 0.82, 0.24, 1];
const CENTER: Rgba = [0.92, 0.93, 0.96, 1];
const UNIT: Vec3[] = [
  [1, 0, 0],
  [0, 1, 0],
  [0, 0, 1],
];
const AXIS_HANDLES: GizmoHandle[] = ["x", "y", "z"];
const PLANES: [number, number, GizmoHandle][] = [
  [0, 1, "xy"],
  [1, 2, "yz"],
  [0, 2, "xz"],
];

export interface GizmoState {
  mode: GizmoMode;
  space: "world" | "local";
  position: Vec3;
  rotation: Quat;
  hovered: GizmoHandle | null;
  active: GizmoHandle | null;
  sizePx?: number;
}

export function axesOf(mode: GizmoMode, space: "world" | "local", rotation: Quat): Vec3[] {
  return mode === "scale" || space === "local" ? UNIT.map((u) => normalize(rotate(rotation, u))) : UNIT;
}

function tint(c: Rgba, a: number): Rgba {
  return [c[0], c[1], c[2], a];
}

function colorFor(handle: GizmoHandle, base: Rgba, g: GizmoState): Rgba {
  if (g.active === handle) return HIGHLIGHT;
  if (g.hovered === handle && !g.active) return HIGHLIGHT;
  if (g.active && g.active !== handle) return tint(base, 0.25);
  return base;
}

/** A cone (arrow head) as side triangles, from `base` along `dir`. */
function cone(base: Vec3, dir: Vec3, len: number, radius: number, color: Rgba, handle: GizmoHandle, view: View): GizmoShape[] {
  const tip = add(base, scale(dir, len));
  const a = normalize(Math.abs(dir[1]) < 0.9 ? cross(dir, [0, 1, 0]) : cross(dir, [1, 0, 0]));
  const b = cross(dir, a);
  const n = 10;
  const ring: Vec3[] = [];
  for (let i = 0; i < n; i++) {
    const t = (i / n) * Math.PI * 2;
    ring.push(add(base, add(scale(a, Math.cos(t) * radius), scale(b, Math.sin(t) * radius))));
  }
  const toEye = view.ortho ? scale(view.forward, -1) : normalize(sub(view.camera.position, tip));
  const shapes: GizmoShape[] = [];
  for (let i = 0; i < n; i++) {
    const p = ring[i]!;
    const q = ring[(i + 1) % n]!;
    const normal = normalize(cross(sub(q, tip), sub(p, tip)));
    const lit = 0.65 + 0.35 * Math.max(0, dot(normal, toEye));
    if (dot(normal, toEye) < -0.05) continue;
    shapes.push({ kind: "polygon", points: [tip, p, q], color: [color[0] * lit, color[1] * lit, color[2] * lit, color[3]], handle });
  }
  return shapes;
}

/** A cube handle (scale) as its visible faces. */
function cube(center: Vec3, axes: Vec3[], half: number, color: Rgba, handle: GizmoHandle, view: View): GizmoShape[] {
  const toEye = view.ortho ? scale(view.forward, -1) : normalize(sub(view.camera.position, center));
  const shapes: GizmoShape[] = [];
  for (let i = 0; i < 3; i++) {
    for (const sgn of [1, -1]) {
      const n = scale(axes[i]!, sgn);
      if (dot(n, toEye) <= 0) continue;
      const u = axes[(i + 1) % 3]!;
      const v = axes[(i + 2) % 3]!;
      const c = add(center, scale(n, half));
      const lit = 0.7 + 0.3 * dot(n, toEye);
      shapes.push({
        kind: "polygon",
        points: [
          add(c, add(scale(u, half), scale(v, half))),
          add(c, add(scale(u, -half), scale(v, half))),
          add(c, add(scale(u, -half), scale(v, -half))),
          add(c, add(scale(u, half), scale(v, -half))),
        ],
        color: [color[0] * lit, color[1] * lit, color[2] * lit, color[3]],
        handle,
      });
    }
  }
  return shapes;
}

function circle(center: Vec3, axis: Vec3, radius: number, segments = 72): Vec3[] {
  const a = normalize(Math.abs(axis[1]) < 0.9 ? cross(axis, [0, 1, 0]) : cross(axis, [1, 0, 0]));
  const b = cross(axis, a);
  const pts: Vec3[] = [];
  for (let i = 0; i <= segments; i++) {
    const t = (i / segments) * Math.PI * 2;
    pts.push(add(center, add(scale(a, Math.cos(t) * radius), scale(b, Math.sin(t) * radius))));
  }
  return pts;
}

export function buildGizmo(g: GizmoState, view: View): Gizmo {
  const sizePx = g.sizePx ?? 96;
  const s = sizePx * view.pixelSize(g.position);
  const p = g.position;
  const axes = axesOf(g.mode, g.space, g.rotation);
  const viewDir = view.ortho ? view.forward : normalize(sub(p, view.camera.position));
  const shapes: GizmoShape[] = [];
  const lines: GizmoShape[] = [];

  if (g.mode === "translate" || g.mode === "scale") {
    // Plane handles first (behind the arrows), translate only.
    if (g.mode === "translate") {
      for (const [i, j, h] of PLANES) {
        const k = 3 - i - j;
        if (Math.abs(dot(axes[k]!, viewDir)) < 0.18) continue;
        const ai = axes[i]!;
        const aj = axes[j]!;
        const o = add(p, add(scale(ai, s * 0.18), scale(aj, s * 0.18)));
        const w = s * 0.2;
        const base = colorFor(h, AXIS_COLORS[k]!, g);
        shapes.push({
          kind: "polygon",
          points: [o, add(o, scale(ai, w)), add(o, add(scale(ai, w), scale(aj, w))), add(o, scale(aj, w))],
          color: tint(base, base === HIGHLIGHT ? 0.55 : 0.3),
          outline: tint(base, 0.9),
          width_px: 1,
          handle: h,
        });
      }
    }
    for (let i = 0; i < 3; i++) {
      const a = axes[i]!;
      if (Math.abs(dot(a, viewDir)) > 0.985) continue;
      const h = AXIS_HANDLES[i]!;
      const c = colorFor(h, AXIS_COLORS[i]!, g);
      const end = add(p, scale(a, s * (g.mode === "translate" ? 0.8 : 0.85)));
      lines.push({ kind: "line", points: [add(p, scale(a, s * 0.12)), end], color: c, width_px: 2.5, handle: h });
      if (g.mode === "translate") shapes.push(...cone(end, a, s * 0.22, s * 0.065, c, h, view));
      else shapes.push(...cube(add(end, scale(a, s * 0.06)), axes, s * 0.06, c, h, view));
    }
    const cc = colorFor("xyz", CENTER, g);
    if (g.mode === "scale") shapes.push(...cube(p, axes, s * 0.08, cc, "xyz", view));
    else {
      const r = view.right;
      const u = view.up;
      const w = s * 0.07;
      shapes.push({
        kind: "polygon",
        points: [add(p, add(scale(r, -w), scale(u, -w))), add(p, add(scale(r, w), scale(u, -w))), add(p, add(scale(r, w), scale(u, w))), add(p, add(scale(r, -w), scale(u, w)))],
        color: tint(cc, 0.25),
        outline: cc,
        width_px: 1.5,
        handle: "xyz",
      });
    }
  } else {
    // Rotation: a ring per axis (front half bright, back half faint) and a view ring.
    const toEye = view.ortho ? scale(view.forward, -1) : normalize(sub(view.camera.position, p));
    for (let i = 0; i < 3; i++) {
      const h = AXIS_HANDLES[i]!;
      const c = colorFor(h, AXIS_COLORS[i]!, g);
      const pts = circle(p, axes[i]!, s * 0.85);
      const facing = (k: number) => dot(sub(scale(add(pts[k]!, pts[k + 1]!), 0.5), p), toEye) >= -s * 0.02;
      const emit = (run: Vec3[], front: boolean) => {
        if (run.length < 2) return;
        if (front) lines.push({ kind: "line", points: run, color: c, width_px: 2.5, handle: h });
        else shapes.push({ kind: "line", points: run, color: tint(c, 0.18), width_px: 1.5 });
      };
      let run: Vec3[] = [pts[0]!];
      let runFront = facing(0);
      for (let k = 0; k + 1 < pts.length; k++) {
        const f = facing(k);
        if (f !== runFront) {
          emit(run, runFront);
          run = [pts[k]!];
          runFront = f;
        }
        run.push(pts[k + 1]!);
      }
      emit(run, runFront);
    }
    const cv = colorFor("xyz", [0.85, 0.87, 0.92, 0.9], g);
    lines.push({ kind: "line", points: circle(p, viewDir, s * 1.02, 96), color: cv, width_px: 1.5, handle: "xyz" });
  }

  // Far to near, lines last so axes stay readable.
  const depth = (sh: GizmoShape) => sh.points.reduce((d, q) => d + view.toView(q)[2], 0) / sh.points.length;
  shapes.sort((a, b) => depth(b) - depth(a));
  return {
    mode: g.mode,
    space: g.space,
    position: p,
    rotation: g.rotation,
    size_px: sizePx,
    hovered: g.hovered,
    active: g.active,
    shapes: [...shapes, ...lines],
  };
}

function distToSegment(px: number, py: number, ax: number, ay: number, bx: number, by: number): number {
  const dx = bx - ax;
  const dy = by - ay;
  const l2 = dx * dx + dy * dy;
  const t = l2 > 0 ? Math.max(0, Math.min(1, ((px - ax) * dx + (py - ay) * dy) / l2)) : 0;
  return Math.hypot(px - (ax + t * dx), py - (ay + t * dy));
}

function inPolygon(x: number, y: number, pts: { x: number; y: number }[]): boolean {
  let inside = false;
  for (let i = 0, j = pts.length - 1; i < pts.length; j = i++) {
    const a = pts[i]!;
    const b = pts[j]!;
    if (a.y > y !== b.y > y && x < ((b.x - a.x) * (y - a.y)) / (b.y - a.y) + a.x) inside = !inside;
  }
  return inside;
}

/** The handle under a pixel, preferring the nearest; null when none is within reach. */
export function hitGizmo(g: Gizmo, view: View, x: number, y: number): GizmoHandle | null {
  let best: GizmoHandle | null = null;
  let bestD = Infinity;
  for (const sh of g.shapes) {
    if (!sh.handle) continue;
    const pts = sh.points.map((q) => view.project(q));
    if (pts.some((q) => !q.visible)) continue;
    let d = Infinity;
    if (sh.kind === "polygon" && inPolygon(x, y, pts)) d = 0;
    for (let i = 0; i + 1 < pts.length; i++) d = Math.min(d, distToSegment(x, y, pts[i]!.x, pts[i]!.y, pts[i + 1]!.x, pts[i + 1]!.y));
    if (sh.kind === "polygon" && pts.length > 2) d = Math.min(d, distToSegment(x, y, pts.at(-1)!.x, pts.at(-1)!.y, pts[0]!.x, pts[0]!.y));
    const reach = (sh.width_px ?? 2) / 2 + 6;
    // Small handles (center, planes) win ties over the long axes crossing them.
    const bias = sh.handle === "xyz" ? -3 : sh.handle.length === 2 ? -1 : 0;
    if (d <= reach && d + bias < bestD) {
      bestD = d + bias;
      best = sh.handle;
    }
  }
  return best;
}

// ---- Dragging ----------------------------------------------------------------------------------

export interface DragStart {
  handle: GizmoHandle;
  mode: GizmoMode;
  space: "world" | "local";
  position: Vec3;
  rotation: Quat;
  /** World size of the gizmo when the drag began. */
  size: number;
  x: number;
  y: number;
  axes: Vec3[];
  /** The grabbed point's parameter or position, for relative motion. */
  t0: number;
  hit0: Vec3 | null;
  viewNormal: Vec3;
}

export interface Snapping {
  enabled: boolean;
  translate: number;
  rotate: number;
  scale: number;
}

export function beginDrag(g: Gizmo, handle: GizmoHandle, view: View, x: number, y: number): DragStart {
  const axes = axesOf(g.mode, g.space, g.rotation);
  const ray = view.ray(x, y);
  const start: DragStart = {
    handle,
    mode: g.mode,
    space: g.space,
    position: g.position,
    rotation: g.rotation,
    size: g.size_px * view.pixelSize(g.position),
    x,
    y,
    axes,
    t0: 0,
    hit0: null,
    viewNormal: view.ortho ? scale(view.forward, -1) : normalize(sub(view.camera.position, g.position)),
  };
  const i = AXIS_HANDLES.indexOf(handle);
  if (g.mode !== "rotate" && i >= 0) start.t0 = closestOnLine(g.position, axes[i]!, ray.origin, ray.dir);
  const normal = planeNormal(start, view);
  if (normal) {
    const t = rayPlane(ray.origin, ray.dir, g.position, normal);
    start.hit0 = t === null ? null : add(ray.origin, scale(ray.dir, t));
  }
  return start;
}

function planeNormal(d: DragStart, view: View): Vec3 | null {
  if (d.mode === "translate") {
    const plane = PLANES.find((p) => p[2] === d.handle);
    if (plane) return d.axes[3 - plane[0] - plane[1]]!;
    if (d.handle === "xyz") return view.ortho ? scale(view.forward, -1) : d.viewNormal;
    return null;
  }
  if (d.mode === "rotate") {
    const i = AXIS_HANDLES.indexOf(d.handle);
    return i >= 0 ? d.axes[i]! : d.viewNormal;
  }
  return null;
}

export interface DragResult {
  /** World translation to add to every dragged entity. */
  translate?: Vec3;
  /** World rotation to apply to every dragged entity (about its own origin). */
  rotate?: Quat;
  rotateDeg?: number;
  /** Per-axis factors to multiply the scale by (in the entity's frame). */
  scale?: Vec3;
}

export function dragTo(d: DragStart, view: View, x: number, y: number, snap: Snapping): DragResult {
  const ray = view.ray(x, y);
  const i = AXIS_HANDLES.indexOf(d.handle);
  if (d.mode === "translate") {
    let delta: Vec3;
    if (i >= 0) {
      const a = d.axes[i]!;
      let t = closestOnLine(d.position, a, ray.origin, ray.dir) - d.t0;
      if (!Number.isFinite(t) || Math.abs(t) > d.size * 400) return {};
      if (snap.enabled && d.space === "local") t = snapTo(t, snap.translate);
      delta = scale(a, t);
    } else {
      const n = planeNormal(d, view)!;
      const t = rayPlane(ray.origin, ray.dir, d.position, n);
      if (t === null || !d.hit0) return {};
      delta = sub(add(ray.origin, scale(ray.dir, t)), d.hit0);
      if (length(delta) > d.size * 400) return {};
    }
    if (snap.enabled && d.space === "world") {
      const target = add(d.position, delta);
      const snapped: Vec3 = [0, 1, 2].map((k) => (Math.abs(delta[k]!) > 1e-9 ? snapTo(target[k]!, snap.translate) : target[k]!)) as Vec3;
      delta = sub(snapped, d.position);
    }
    return { translate: delta };
  }
  if (d.mode === "rotate") {
    const axis = i >= 0 ? d.axes[i]! : d.viewNormal;
    let angle: number;
    const t = rayPlane(ray.origin, ray.dir, d.position, axis);
    const edgeOn = Math.abs(dot(ray.dir, axis)) < 0.12;
    if (t !== null && d.hit0 && !edgeOn) {
      const v0 = sub(d.hit0, d.position);
      const v1 = sub(add(ray.origin, scale(ray.dir, t)), d.position);
      angle = Math.atan2(dot(cross(v0, v1), axis), dot(v0, v1));
    } else {
      angle = ((x - d.x - (y - d.y)) * 0.6 * DEG) as number;
    }
    let deg = angle / DEG;
    if (snap.enabled) deg = snapTo(deg, snap.rotate);
    return { rotate: qnormalize(qaxis(axis, deg * DEG)), rotateDeg: deg };
  }
  // Scale.
  if (i >= 0) {
    const a = d.axes[i]!;
    const t = closestOnLine(d.position, a, ray.origin, ray.dir);
    let f = 1 + (t - d.t0) / Math.max(1e-6, d.size * 0.85);
    if (snap.enabled) f = Math.max(snap.scale, snapTo(f, snap.scale));
    const out: Vec3 = [1, 1, 1];
    out[i] = f;
    return { scale: out };
  }
  let f = Math.max(0.01, 1 + (x - d.x - (y - d.y)) / 120);
  if (snap.enabled) f = Math.max(snap.scale, snapTo(f, snap.scale));
  return { scale: [f, f, f] };
}

/** Applies a world rotation delta to an orientation. */
export function applyRotation(delta: Quat, rotation: Quat): Quat {
  return qnormalize(qmul(delta, rotation));
}
