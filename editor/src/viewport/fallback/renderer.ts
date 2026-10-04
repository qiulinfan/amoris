// The fallback viewport: a Canvas 2D view of the world data the editor already holds, used until the
// engine's wasm renderer is served. It implements the same `Viewport` interface, so the editor's
// camera, picking, selection and gizmo code is the code that will drive the real renderer.
// Flat-shaded low-poly stand-ins sorted back to front, an animated sea from the `Sea` component's
// waves, a fading grid, light and camera icons, and the gizmo on top.

import { View, type Camera } from "../camera";
import type { Gizmo, Overlays, PickHit, Viewport } from "../engine";
import { add, asQuat, asVec3, cross, dot, IDENTITY, length, lerp3, normalize, qslerp, rotate, scale, sub, type Quat, type Vec3 } from "../math";
import { meshFor, primitive, sloop, type Mesh } from "./meshes";

export interface FallbackEntity {
  id: number;
  name: string | null;
  components: Record<string, unknown>;
}

export interface FallbackSource {
  entities(): Iterable<FallbackEntity>;
  /** Seconds, for the waves. */
  time(): number;
  /** Ease toward new transforms (the world is playing) instead of jumping. */
  smooth(): boolean;
}

interface DrawFace {
  entity: number;
  pts: { x: number; y: number }[];
  depth: number;
  fill: string;
  stroke?: string;
  plane: { p: Vec3; n: Vec3 };
}

interface Icon {
  entity: number;
  x: number;
  y: number;
  r: number;
  depth: number;
  kind: "sun" | "light" | "camera" | "marker";
  color: string;
}

const SKY_TOP = [24, 29, 39];
const SKY_HORIZON = [58, 68, 84];
const GROUND = [21, 24, 30];
const FOG = [52, 62, 77];
const SEA_DEEP: Vec3 = [0.06, 0.2, 0.3];
const SEA_SHALLOW: Vec3 = [0.16, 0.42, 0.52];

function rgb(c: number[], a = 1): string {
  return a >= 1 ? `rgb(${c[0]! | 0},${c[1]! | 0},${c[2]! | 0})` : `rgba(${c[0]! | 0},${c[1]! | 0},${c[2]! | 0},${a})`;
}

function srgb(c: number): number {
  const v = c <= 0.0031308 ? 12.92 * c : 1.055 * Math.pow(Math.max(0, c), 1 / 2.4) - 0.055;
  return Math.max(0, Math.min(255, v * 255));
}

function mix(a: number[], b: number[], t: number): number[] {
  return a.map((x, i) => x + (b[i]! - x) * t);
}

function bearing(deg: number): Vec3 {
  const r = (deg * Math.PI) / 180;
  return [Math.sin(r), 0, -Math.cos(r)];
}

interface Wave {
  length: number;
  height: number;
  toward_deg: number;
  phase: number;
}

function waveHeight(waves: Wave[], level: number, x: number, z: number, t: number): number {
  let h = level;
  for (const w of waves) {
    const k = (2 * Math.PI) / Math.max(0.1, w.length);
    const omega = Math.sqrt(9.81 * k);
    const d = bearing(w.toward_deg);
    h += (w.height / 2) * Math.sin(k * (d[0] * x + d[2] * z) - omega * t + w.phase);
  }
  return h;
}

/** Clips a camera-space polygon against the near plane (z = near). */
function clipNear(poly: Vec3[], near: number): Vec3[] {
  const out: Vec3[] = [];
  for (let i = 0; i < poly.length; i++) {
    const a = poly[i]!;
    const b = poly[(i + 1) % poly.length]!;
    const ain = a[2] >= near;
    const bin = b[2] >= near;
    if (ain) out.push(a);
    if (ain !== bin) {
      const t = (near - a[2]) / (b[2] - a[2]);
      out.push([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, near]);
    }
  }
  return out;
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

class FallbackViewport implements Viewport {
  private readonly ctx: CanvasRenderingContext2D;
  private camera: Camera | null = null;
  private width = 1;
  private height = 1;
  private dpr = 1;
  private gizmo: Gizmo | null = null;
  private selection = new Set<number>();
  private hovered: number | null = null;
  private overlays: Overlays = { grid: true, axes: true };
  private raf = 0;
  private disposed = false;
  private faces: DrawFace[] = [];
  private icons: Icon[] = [];
  private shown = new Map<number, { pos: Vec3; rot: Quat }>();
  private lastFrame = performance.now();

  constructor(
    private readonly canvas: HTMLCanvasElement,
    private readonly source: FallbackSource,
  ) {
    const ctx = canvas.getContext("2d", { alpha: false });
    if (!ctx) throw new Error("Canvas 2D is unavailable");
    this.ctx = ctx;
    const loop = () => {
      if (this.disposed) return;
      this.draw();
      this.raf = requestAnimationFrame(loop);
    };
    this.raf = requestAnimationFrame(loop);
  }

  setCamera(camera: Camera) {
    this.camera = camera;
  }

  resize(width: number, height: number, dpr: number) {
    this.width = Math.max(1, width);
    this.height = Math.max(1, height);
    this.dpr = dpr;
    this.canvas.width = Math.round(this.width * dpr);
    this.canvas.height = Math.round(this.height * dpr);
  }

  setGizmo(gizmo: Gizmo | null) {
    this.gizmo = gizmo;
  }

  setSelection(entities: readonly number[], hovered: number | null) {
    this.selection = new Set(entities);
    this.hovered = hovered;
  }

  setOverlays(overlays: Overlays) {
    this.overlays = overlays;
  }

  async pick(x: number, y: number): Promise<PickHit | null> {
    const view = this.camera ? new View(this.camera, this.width, this.height) : null;
    for (let i = this.icons.length - 1; i >= 0; i--) {
      const ic = this.icons[i]!;
      if (Math.hypot(x - ic.x, y - ic.y) <= ic.r + 3) return { entity: ic.entity };
    }
    for (let i = this.faces.length - 1; i >= 0; i--) {
      const f = this.faces[i]!;
      if (f.entity < 0 || !inPolygon(x, y, f.pts)) continue;
      if (view) {
        const ray = view.ray(x, y);
        const d = dot(ray.dir, f.plane.n);
        if (Math.abs(d) > 1e-6) {
          const t = dot(sub(f.plane.p, ray.origin), f.plane.n) / d;
          if (t > 0) return { entity: f.entity, position: add(ray.origin, scale(ray.dir, t)) };
        }
      }
      return { entity: f.entity };
    }
    return null;
  }

  dispose() {
    this.disposed = true;
    cancelAnimationFrame(this.raf);
  }

  // ---- Drawing ---------------------------------------------------------------------------------

  private draw() {
    const now = performance.now();
    const dt = Math.min(0.1, (now - this.lastFrame) / 1000);
    this.lastFrame = now;
    const ctx = this.ctx;
    ctx.setTransform(this.dpr, 0, 0, this.dpr, 0, 0);
    if (!this.camera) {
      ctx.fillStyle = rgb(GROUND);
      ctx.fillRect(0, 0, this.width, this.height);
      return;
    }
    const view = new View(this.camera, this.width, this.height);
    const entities = [...this.source.entities()];
    const t = this.source.time();
    const horizonY = this.horizon(view);

    this.background(view, horizonY);
    const sea = entities.find((e) => e.components.Sea)?.components.Sea as { level?: number; waves?: Wave[] } | undefined;
    if (sea) this.sea(view, sea.waves ?? [], sea.level ?? 0, t, horizonY);
    if (this.overlays.grid) this.grid(view, sea ? (sea.level ?? 0) + 0.02 : 0);

    const sun = entities.find((e) => (e.components.Light as { kind?: string } | undefined)?.kind === "Directional");
    const lightDir = sun ? normalize(rotate(asQuat((sun.components.Transform as { rotation?: unknown } | undefined)?.rotation), [0, 0, -1])) : normalize([-0.45, -0.8, -0.4]);

    this.faces = [];
    this.icons = [];
    const k = this.source.smooth() ? 1 - Math.exp(-dt * 16) : 1;
    const seen = new Set<number>();
    for (const e of entities) {
      seen.add(e.id);
      this.entity(view, e, lightDir, k);
    }
    for (const id of [...this.shown.keys()]) if (!seen.has(id)) this.shown.delete(id);

    this.faces.sort((a, b) => b.depth - a.depth);
    ctx.lineJoin = "round";
    for (const f of this.faces) {
      ctx.beginPath();
      f.pts.forEach((p, i) => (i ? ctx.lineTo(p.x, p.y) : ctx.moveTo(p.x, p.y)));
      ctx.closePath();
      ctx.fillStyle = f.fill;
      ctx.fill();
      ctx.strokeStyle = f.fill;
      ctx.lineWidth = 0.6;
      ctx.stroke();
    }
    // Selection and hover outlines over the faces.
    for (const f of this.faces) {
      const sel = this.selection.has(f.entity);
      const hov = this.hovered === f.entity && !sel;
      if (!sel && !hov) continue;
      ctx.beginPath();
      f.pts.forEach((p, i) => (i ? ctx.lineTo(p.x, p.y) : ctx.moveTo(p.x, p.y)));
      ctx.closePath();
      ctx.strokeStyle = sel ? "rgba(255,166,64,0.95)" : "rgba(130,180,255,0.7)";
      ctx.lineWidth = sel ? 1.4 : 1;
      ctx.stroke();
    }
    this.icons.sort((a, b) => b.depth - a.depth);
    for (const ic of this.icons) this.icon(ic);
    this.labels(view, entities);
    if (this.gizmo) this.drawGizmo(view, this.gizmo);
  }

  private horizon(view: View): number {
    const f = view.forward;
    const h = normalize([f[0], 0, f[2]]);
    if (length(h) < 1e-6) return f[1] < 0 ? -1e6 : 1e6;
    const far = add(view.camera.position, scale(h, 1e5));
    const v = view.toView(far);
    if (v[2] <= 0) return f[1] < 0 ? -1e6 : 1e6;
    return view.viewToScreen(v).y;
  }

  private background(_view: View, horizonY: number) {
    const ctx = this.ctx;
    const hy = Math.max(-this.height, Math.min(this.height * 2, horizonY));
    const g = ctx.createLinearGradient(0, Math.min(0, hy - this.height), 0, hy);
    g.addColorStop(0, rgb(SKY_TOP));
    g.addColorStop(1, rgb(SKY_HORIZON));
    ctx.fillStyle = g;
    ctx.fillRect(0, 0, this.width, Math.max(0, Math.min(this.height, hy)));
    if (hy < this.height) {
      const gg = ctx.createLinearGradient(0, hy, 0, this.height + Math.max(0, this.height - hy));
      gg.addColorStop(0, rgb(mix(SKY_HORIZON, GROUND, 0.5)));
      gg.addColorStop(1, rgb(GROUND));
      ctx.fillStyle = gg;
      ctx.fillRect(0, Math.max(0, hy), this.width, this.height - Math.max(0, hy));
    }
  }

  private sea(view: View, waves: Wave[], level: number, t: number, horizonY: number) {
    const ctx = this.ctx;
    // The far sea as a band to the horizon, then a tessellated near sea with the waves.
    if (horizonY < this.height) {
      const g = ctx.createLinearGradient(0, Math.max(0, horizonY), 0, this.height);
      g.addColorStop(0, rgb(mix(FOG, SEA_SHALLOW.map(srgb), 0.35)));
      g.addColorStop(1, rgb(SEA_DEEP.map(srgb)));
      ctx.fillStyle = g;
      ctx.fillRect(0, Math.max(0, horizonY), this.width, this.height - Math.max(0, horizonY));
    }
    const target = view.camera.target;
    const camDist = length(sub(view.camera.position, target));
    const cell = Math.max(1, Math.min(6, camDist / 12));
    const n = 36;
    const half = (n * cell) / 2;
    const ox = Math.round(target[0] / cell) * cell - half;
    const oz = Math.round(target[2] / cell) * cell - half;
    const hs: number[] = [];
    for (let j = 0; j <= n; j++) for (let i = 0; i <= n; i++) hs.push(waveHeight(waves, level, ox + i * cell, oz + j * cell, t));
    const quads: { pts: { x: number; y: number }[]; depth: number; fill: string }[] = [];
    const toEyeBase = view.camera.position;
    for (let j = 0; j < n; j++) {
      for (let i = 0; i < n; i++) {
        const corners: Vec3[] = [
          [ox + i * cell, hs[j * (n + 1) + i]!, oz + j * cell],
          [ox + (i + 1) * cell, hs[j * (n + 1) + i + 1]!, oz + j * cell],
          [ox + (i + 1) * cell, hs[(j + 1) * (n + 1) + i + 1]!, oz + (j + 1) * cell],
          [ox + i * cell, hs[(j + 1) * (n + 1) + i]!, oz + (j + 1) * cell],
        ];
        const vs = clipNear(corners.map((c) => view.toView(c)), view.camera.near);
        if (vs.length < 3) continue;
        const pts = vs.map((v) => view.viewToScreen(v));
        if (pts.every((p) => p.x < -50 || p.x > this.width + 50 || p.y < -50 || p.y > this.height + 50)) continue;
        const nrm = normalize(cross(sub(corners[3]!, corners[0]!), sub(corners[1]!, corners[0]!)));
        const c = scale(add(corners[0]!, corners[2]!), 0.5);
        const toEye = normalize(sub(toEyeBase, c));
        const fres = Math.pow(1 - Math.max(0, dot(nrm, toEye)), 3);
        const slope = 1 - nrm[1];
        const base = mix(SEA_DEEP, SEA_SHALLOW, Math.min(1, slope * 6 + fres * 0.6)).map(srgb);
        const depth = vs.reduce((s, v) => s + v[2], 0) / vs.length;
        const edge = Math.max(Math.abs(c[0] - target[0]), Math.abs(c[2] - target[2])) / half;
        const fogT = Math.min(1, Math.max(0, (edge - 0.55) / 0.45)) * 0.85 + Math.min(0.5, depth / 900);
        quads.push({ pts, depth, fill: rgb(mix(base, mix(FOG, SEA_SHALLOW.map(srgb), 0.35), Math.min(1, fogT))) });
      }
    }
    quads.sort((a, b) => b.depth - a.depth);
    for (const q of quads) {
      ctx.beginPath();
      q.pts.forEach((p, i) => (i ? ctx.lineTo(p.x, p.y) : ctx.moveTo(p.x, p.y)));
      ctx.closePath();
      ctx.fillStyle = q.fill;
      ctx.fill();
      ctx.strokeStyle = q.fill;
      ctx.lineWidth = 0.75;
      ctx.stroke();
    }
  }

  private line3(view: View, a: Vec3, b: Vec3): [{ x: number; y: number }, { x: number; y: number }] | null {
    let va = view.toView(a);
    let vb = view.toView(b);
    const near = view.camera.near;
    if (!view.ortho) {
      if (va[2] < near && vb[2] < near) return null;
      if (va[2] < near) va = lerp3(va, vb, (near - va[2]) / (vb[2] - va[2]));
      else if (vb[2] < near) vb = lerp3(vb, va, (near - vb[2]) / (va[2] - vb[2]));
    }
    return [view.viewToScreen(va), view.viewToScreen(vb)];
  }

  private grid(view: View, y: number) {
    const ctx = this.ctx;
    const target = view.camera.target;
    const dist = length(sub(view.camera.position, target));
    const step = dist > 120 ? 10 : dist > 40 ? 5 : 1;
    const major = step * 10;
    const radius = Math.max(20, Math.min(400, dist * 2.2));
    const cx = Math.round(target[0] / step) * step;
    const cz = Math.round(target[2] / step) * step;
    ctx.lineWidth = 1;
    const draw = (a: Vec3, b: Vec3, color: string, w = 1) => {
      const s = this.line3(view, a, b);
      if (!s) return;
      ctx.strokeStyle = color;
      ctx.lineWidth = w;
      ctx.beginPath();
      ctx.moveTo(s[0].x, s[0].y);
      ctx.lineTo(s[1].x, s[1].y);
      ctx.stroke();
    };
    const segs = 8;
    for (let k = -radius; k <= radius; k += step) {
      for (const axis of [0, 1]) {
        const coord = (axis === 0 ? cx : cz) + k;
        if (Math.abs(coord) < 1e-6) continue;
        const isMajor = Math.abs(coord % major) < 1e-6;
        // Fade with distance from the target, in segments.
        for (let s = 0; s < segs; s++) {
          const u0 = -radius + (2 * radius * s) / segs;
          const u1 = -radius + (2 * radius * (s + 1)) / segs;
          const mid = (u0 + u1) / 2;
          const fade = 1 - Math.min(1, Math.hypot(k, mid) / radius);
          if (fade <= 0.02) continue;
          const alpha = (isMajor ? 0.22 : 0.09) * fade;
          const a: Vec3 = axis === 0 ? [coord, y, cz + u0] : [cx + u0, y, coord];
          const b: Vec3 = axis === 0 ? [coord, y, cz + u1] : [cx + u1, y, coord];
          draw(a, b, `rgba(200,212,232,${alpha.toFixed(3)})`);
        }
      }
    }
    if (this.overlays.axes) {
      draw([-radius * 2 + cx, y, 0], [radius * 2 + cx, y, 0], "rgba(239,83,80,0.55)", 1.4);
      draw([0, y, -radius * 2 + cz], [0, y, radius * 2 + cz], "rgba(66,165,245,0.55)", 1.4);
    }
  }

  private entity(view: View, e: FallbackEntity, lightDir: Vec3, k: number) {
    const c = e.components;
    const tr = c.Transform as { position?: unknown; rotation?: unknown } | undefined;
    if (!tr) return;
    const targetPos = asVec3(tr.position);
    const targetRot = asQuat(tr.rotation ?? IDENTITY);
    let shown = this.shown.get(e.id);
    if (!shown || k >= 1 || length(sub(shown.pos, targetPos)) > 25) {
      shown = { pos: targetPos, rot: targetRot };
    } else {
      shown = { pos: lerp3(shown.pos, targetPos, k), rot: qslerp(shown.rot, targetRot, k) };
    }
    this.shown.set(e.id, shown);
    const { pos, rot } = shown;

    const light = c.Light as { kind?: string; color?: number[] } | undefined;
    if (light) {
      const p = view.project(pos);
      if (p.visible) {
        const col = (light.color ?? [1, 0.95, 0.8]).map(srgb);
        this.icons.push({ entity: e.id, x: p.x, y: p.y, r: 10, depth: p.depth, kind: light.kind === "Directional" ? "sun" : "light", color: rgb(col) });
        if (this.selection.has(e.id)) {
          const dir = rotate(rot, [0, 0, -1]);
          const s = this.line3(view, pos, add(pos, scale(dir, light.kind === "Directional" ? 6 : 3)));
          if (s) this.dashed(s, "rgba(255,214,102,0.8)");
        }
      }
      return;
    }
    const cam = c.Camera as { fov_deg?: number } | undefined;
    if (cam) {
      this.frustum(view, e.id, pos, rot, cam.fov_deg ?? 60);
      return;
    }

    let mesh: Mesh | null = null;
    let size: Vec3 = [1, 1, 1];
    let color: Vec3 = [0.7, 0.72, 0.76];
    const model = c.Model as { mesh?: string; scale?: unknown; color?: number[]; visible?: boolean } | undefined;
    const boat = c.Boat as { boom_deg?: number; hoist_now?: number } | undefined;
    if (model && model.visible !== false) {
      const m = meshFor(model.mesh ?? "box");
      mesh = m.name === "sloop" ? sloop(boat?.boom_deg ?? 0, boat?.hoist_now ?? 1) : m.mesh;
      size = asVec3(model.scale, [1, 1, 1]);
      if (model.color) color = [model.color[0] ?? 0.7, model.color[1] ?? 0.7, model.color[2] ?? 0.7];
    } else if (boat) {
      mesh = sloop(boat.boom_deg ?? 0, boat.hoist_now ?? 1);
      color = [0.86, 0.88, 0.9];
    } else {
      const shape = (c.Collider as { shape?: Record<string, Record<string, unknown>> } | undefined)?.shape;
      if (shape?.Cuboid) {
        mesh = primitive("box");
        size = scale(asVec3(shape.Cuboid.half_extents, [0.5, 0.5, 0.5]), 2);
      } else if (shape?.Ball) {
        mesh = primitive("sphere");
        const r = Number(shape.Ball.radius) || 0.5;
        size = [r * 2, r * 2, r * 2];
      } else if (shape?.Capsule) {
        mesh = primitive("cylinder");
        const r = Number(shape.Capsule.radius) || 0.5;
        size = [r * 2, (Number(shape.Capsule.half_height) || 0.5) * 2 + r * 2, r * 2];
      }
    }
    if (!mesh) {
      const p = view.project(pos);
      if (p.visible) this.icons.push({ entity: e.id, x: p.x, y: p.y, r: 6, depth: p.depth, kind: "marker", color: "rgba(200,208,224,0.9)" });
      return;
    }
    const world = mesh.verts.map((v) => add(pos, rotate(rot, [v[0] * size[0], v[1] * size[1], v[2] * size[2]])));
    const eye = view.camera.position;
    for (const f of mesh.faces) {
      const ws = f.idx.map((i) => world[i]!);
      let n = normalize(cross(sub(ws[1]!, ws[0]!), sub(ws[2]!, ws[0]!)));
      const center = ws.reduce((a, b) => add(a, b), [0, 0, 0] as Vec3).map((x) => x / ws.length) as Vec3;
      const toEye = view.ortho ? scale(view.forward, -1) : sub(eye, center);
      if (dot(n, toEye) < 0) {
        if (!f.doubleSided) continue;
        n = scale(n, -1);
      }
      const vs = clipNear(ws.map((w) => view.toView(w)), view.ortho ? -1e9 : view.camera.near);
      if (vs.length < 3) continue;
      const pts = vs.map((v) => view.viewToScreen(v));
      const base = f.color ?? color;
      const lambert = Math.max(0, dot(n, scale(lightDir, -1)));
      const lit = 0.38 + 0.62 * lambert + 0.08 * n[1];
      const depth = vs.reduce((s, v) => s + v[2], 0) / vs.length;
      const fog = Math.min(0.6, Math.max(0, (depth - 60) / 500));
      const col = mix(base.map((x) => srgb(Math.min(1, x * lit))), FOG, fog);
      this.faces.push({ entity: e.id, pts, depth, fill: rgb(col), plane: { p: ws[0]!, n } });
    }
  }

  private frustum(view: View, id: number, pos: Vec3, rot: Quat, fov: number) {
    const ctx = this.ctx;
    const len = 1.6;
    const h = Math.tan(((fov / 2) * Math.PI) / 180) * len;
    const w = h * (16 / 9);
    const fwd = rotate(rot, [0, 0, -1]);
    const right = rotate(rot, [1, 0, 0]);
    const up = rotate(rot, [0, 1, 0]);
    const c = add(pos, scale(fwd, len));
    const corners = [
      add(c, add(scale(right, -w), scale(up, h))),
      add(c, add(scale(right, w), scale(up, h))),
      add(c, add(scale(right, w), scale(up, -h))),
      add(c, add(scale(right, -w), scale(up, -h))),
    ];
    const sel = this.selection.has(id);
    ctx.strokeStyle = sel ? "rgba(255,166,64,0.95)" : "rgba(196,206,226,0.75)";
    ctx.lineWidth = sel ? 1.5 : 1;
    const segs: [Vec3, Vec3][] = [
      ...corners.map((q) => [pos, q] as [Vec3, Vec3]),
      ...corners.map((q, i) => [q, corners[(i + 1) % 4]!] as [Vec3, Vec3]),
      [add(c, scale(up, h * 1.15)), add(c, add(scale(up, h * 1.6), scale(right, 0)))],
    ];
    for (const [a, b] of segs) {
      const s = this.line3(view, a, b);
      if (!s) continue;
      ctx.beginPath();
      ctx.moveTo(s[0].x, s[0].y);
      ctx.lineTo(s[1].x, s[1].y);
      ctx.stroke();
    }
    const p = view.project(pos);
    if (p.visible) this.icons.push({ entity: id, x: p.x, y: p.y, r: 9, depth: p.depth, kind: "camera", color: "rgb(196,206,226)" });
  }

  private dashed(s: [{ x: number; y: number }, { x: number; y: number }], color: string) {
    const ctx = this.ctx;
    ctx.save();
    ctx.setLineDash([4, 4]);
    ctx.strokeStyle = color;
    ctx.lineWidth = 1.2;
    ctx.beginPath();
    ctx.moveTo(s[0].x, s[0].y);
    ctx.lineTo(s[1].x, s[1].y);
    ctx.stroke();
    ctx.restore();
  }

  private icon(ic: Icon) {
    const ctx = this.ctx;
    const sel = this.selection.has(ic.entity);
    const hov = this.hovered === ic.entity;
    ctx.save();
    ctx.translate(ic.x, ic.y);
    ctx.fillStyle = sel ? "rgba(255,166,64,0.22)" : hov ? "rgba(130,180,255,0.18)" : "rgba(14,17,22,0.55)";
    ctx.strokeStyle = sel ? "rgba(255,166,64,0.95)" : "rgba(220,226,238,0.55)";
    ctx.lineWidth = 1.2;
    if (ic.kind === "marker") {
      ctx.beginPath();
      ctx.moveTo(0, -ic.r);
      ctx.lineTo(ic.r, 0);
      ctx.lineTo(0, ic.r);
      ctx.lineTo(-ic.r, 0);
      ctx.closePath();
      ctx.fill();
      ctx.stroke();
      ctx.restore();
      return;
    }
    ctx.beginPath();
    ctx.arc(0, 0, ic.r + 3, 0, Math.PI * 2);
    ctx.fill();
    ctx.stroke();
    ctx.strokeStyle = ic.color;
    ctx.fillStyle = ic.color;
    if (ic.kind === "sun") {
      ctx.beginPath();
      ctx.arc(0, 0, 4, 0, Math.PI * 2);
      ctx.fill();
      ctx.lineWidth = 1.5;
      for (let i = 0; i < 8; i++) {
        const a = (i / 8) * Math.PI * 2;
        ctx.beginPath();
        ctx.moveTo(Math.cos(a) * 6.5, Math.sin(a) * 6.5);
        ctx.lineTo(Math.cos(a) * 9, Math.sin(a) * 9);
        ctx.stroke();
      }
    } else if (ic.kind === "light") {
      ctx.beginPath();
      ctx.arc(0, -1.5, 4.5, 0, Math.PI * 2);
      ctx.fill();
      ctx.fillRect(-2.5, 3, 5, 3);
    } else {
      ctx.fillRect(-6, -4, 8.5, 8);
      ctx.beginPath();
      ctx.moveTo(2.5, -1.5);
      ctx.lineTo(7, -4);
      ctx.lineTo(7, 4);
      ctx.lineTo(2.5, 1.5);
      ctx.closePath();
      ctx.fill();
    }
    ctx.restore();
  }

  private labels(view: View, entities: FallbackEntity[]) {
    const ctx = this.ctx;
    ctx.font = "500 11px 'Inter Variable', system-ui, sans-serif";
    ctx.textBaseline = "middle";
    for (const e of entities) {
      if (!this.selection.has(e.id) || !e.name) continue;
      const shown = this.shown.get(e.id);
      if (!shown) continue;
      const p = view.project(add(shown.pos, [0, 1.2, 0]));
      if (!p.visible) continue;
      const w = ctx.measureText(e.name).width + 12;
      const x = p.x - w / 2;
      const y = p.y - 26;
      ctx.fillStyle = "rgba(12,14,19,0.78)";
      ctx.beginPath();
      ctx.roundRect(x, y - 9, w, 18, 5);
      ctx.fill();
      ctx.fillStyle = "rgba(255,190,110,1)";
      ctx.fillText(e.name, x + 6, y);
    }
  }

  private drawGizmo(view: View, g: Gizmo) {
    const ctx = this.ctx;
    const col = (c: number[]) => `rgba(${Math.round(c[0]! * 255)},${Math.round(c[1]! * 255)},${Math.round(c[2]! * 255)},${c[3] ?? 1})`;
    ctx.lineCap = "round";
    for (const sh of g.shapes) {
      const pts = sh.points.map((q) => view.project(q));
      if (pts.some((q) => !q.visible)) continue;
      ctx.beginPath();
      pts.forEach((q, i) => (i ? ctx.lineTo(q.x, q.y) : ctx.moveTo(q.x, q.y)));
      if (sh.kind === "polygon") {
        ctx.closePath();
        ctx.fillStyle = col(sh.color);
        ctx.fill();
        if (sh.outline) {
          ctx.strokeStyle = col(sh.outline);
          ctx.lineWidth = sh.width_px ?? 1;
          ctx.stroke();
        }
      } else {
        ctx.strokeStyle = col(sh.color);
        ctx.lineWidth = sh.width_px ?? 2;
        ctx.stroke();
      }
    }
    ctx.lineCap = "butt";
  }
}

export function createFallbackViewport(canvas: HTMLCanvasElement, source: FallbackSource): Viewport {
  return new FallbackViewport(canvas, source);
}
