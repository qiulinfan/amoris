// The viewport's controller, independent of React: it owns the orbit camera, feeds the renderer the
// camera, selection, overlays and gizmo every frame, and turns pointer input into camera moves,
// picks, rectangle selections and gizmo drags (live `world.edit`s through `LiveEdit`).

import { LiveEdit } from "../../actions/world";
import type { EntityId, Quat, Vec3 } from "../../host/protocol";
import { useSelection } from "../../state/selection";
import { useSession } from "../../state/session";
import { dragState, useViewport } from "../../state/viewport";
import { entityName, useWorld } from "../../state/world";
import { cameraOf, clampPitch, DEFAULT_ORBIT, View, type Orbit } from "../../viewport/camera";
import type { Gizmo, GizmoHandle, Viewport } from "../../viewport/engine";
import { applyRotation, beginDrag, buildGizmo, dragTo, hitGizmo, type DragStart } from "../../viewport/gizmo";
import { add, asQuat, asVec3, DEG, IDENTITY, length, rayPlane, scale, sub } from "../../viewport/math";

const ORBIT_KEY = "amoris.editor.camera.v1";

interface DragTarget {
  id: EntityId;
  position: Vec3;
  rotation: Quat;
  arrays: boolean;
  scale: Vec3 | null;
  scaleComponent: "Transform" | "Model" | null;
}

type Mode =
  | { kind: "none" }
  | { kind: "orbit"; x: number; y: number; moved: boolean; button: number }
  | { kind: "pan"; x: number; y: number }
  | { kind: "click"; x: number; y: number; additive: boolean }
  | { kind: "rect"; x0: number; y0: number; x1: number; y1: number; additive: boolean }
  | { kind: "gizmo"; start: DragStart; targets: DragTarget[]; edit: LiveEdit; snapInvert: boolean };

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

function loadOrbit(): Orbit {
  try {
    const raw = localStorage.getItem(ORBIT_KEY);
    if (raw) return { ...DEFAULT_ORBIT, ...(JSON.parse(raw) as Partial<Orbit>) };
  } catch {
    // fall through
  }
  return { ...DEFAULT_ORBIT };
}

export class ViewportController {
  engine: Viewport | null = null;
  orbit: Orbit = loadOrbit();
  width = 1;
  height = 1;
  private mode: Mode = { kind: "none" };
  private gizmo: Gizmo | null = null;
  private hoverHandle: GizmoHandle | null = null;
  private raf = 0;
  private anim: { from: Orbit; to: Orbit; t0: number; ms: number } | null = null;
  private saveTimer: ReturnType<typeof setTimeout> | null = null;
  private lastHoverPick = 0;
  private listeners = new Set<() => void>();
  rect: Rect | null = null;

  start() {
    const loop = () => {
      this.frame();
      this.raf = requestAnimationFrame(loop);
    };
    this.raf = requestAnimationFrame(loop);
  }

  stop() {
    cancelAnimationFrame(this.raf);
  }

  /** Called when the camera or the selection rectangle changes (overlays re-render). */
  subscribe(fn: () => void): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  private changed() {
    this.listeners.forEach((l) => l());
    if (this.saveTimer) clearTimeout(this.saveTimer);
    this.saveTimer = setTimeout(() => {
      try {
        localStorage.setItem(ORBIT_KEY, JSON.stringify(this.orbit));
      } catch {
        // storage full or disabled
      }
      useViewport.getState().set({ target: this.orbit.target.map((v) => Math.round(v * 100) / 100) as Vec3 });
    }, 200);
  }

  view(): View {
    return new View(cameraOf(this.orbit), this.width, this.height);
  }

  resize(w: number, h: number) {
    this.width = Math.max(1, w);
    this.height = Math.max(1, h);
    this.engine?.resize(this.width, this.height, window.devicePixelRatio || 1);
  }

  attach(engine: Viewport) {
    this.engine = engine;
    engine.resize(this.width, this.height, window.devicePixelRatio || 1);
    engine.setCamera(cameraOf(this.orbit));
    const sel = useSelection.getState();
    engine.setSelection(sel.ids, sel.hovered);
    const vp = useViewport.getState();
    engine.setOverlays({ grid: vp.grid, axes: true });
  }

  // ---- Per frame -------------------------------------------------------------------------------

  private frame() {
    if (this.anim) {
      const t = Math.min(1, (performance.now() - this.anim.t0) / this.anim.ms);
      const e = 1 - Math.pow(1 - t, 3);
      const { from, to } = this.anim;
      this.orbit = {
        ...to,
        target: add(from.target, scale(sub(to.target, from.target), e)),
        yaw: from.yaw + (to.yaw - from.yaw) * e,
        pitch: from.pitch + (to.pitch - from.pitch) * e,
        distance: from.distance * Math.pow(to.distance / from.distance, e),
      };
      if (t >= 1) this.anim = null;
      this.changed();
    }
    if (!this.engine) return;
    this.engine.setCamera(cameraOf(this.orbit));
    this.gizmo = this.buildGizmo();
    this.engine.setGizmo(this.gizmo);
  }

  /** The dragged entities: those selected that have a Transform. */
  private targets(): DragTarget[] {
    const w = useWorld.getState();
    const out: DragTarget[] = [];
    for (const id of useSelection.getState().ids) {
      const c = { ...(w.entities[id]?.components ?? {}), ...(w.preview[id] ?? {}) };
      const t = c.Transform as { position?: unknown; rotation?: unknown; scale?: unknown } | undefined;
      if (!t || t.position === undefined) continue;
      const model = c.Model as { scale?: unknown } | undefined;
      const scaleComponent = t.scale !== undefined ? "Transform" : model?.scale !== undefined ? "Model" : null;
      out.push({
        id,
        position: asVec3(t.position),
        rotation: t.rotation !== undefined ? asQuat(t.rotation) : IDENTITY,
        arrays: Array.isArray(t.position),
        scale: scaleComponent === "Transform" ? asVec3(t.scale, [1, 1, 1]) : scaleComponent === "Model" ? asVec3(model!.scale, [1, 1, 1]) : null,
        scaleComponent,
      });
    }
    return out;
  }

  private buildGizmo(): Gizmo | null {
    const vp = useViewport.getState();
    if (vp.tool === "select") return null;
    const targets = this.mode.kind === "gizmo" ? null : this.targets();
    let position: Vec3;
    let rotation: Quat;
    if (this.mode.kind === "gizmo") {
      // Follow the drag: the first target's current (previewed) transform.
      const live = this.targets();
      if (!live.length) return null;
      const n = live.length;
      position = vp.tool === "translate" ? scale(live.reduce((a, t) => add(a, t.position), [0, 0, 0] as Vec3), 1 / n) : live[0]!.position;
      rotation = this.mode.start.space === "local" || vp.tool === "scale" ? live[live.length - 1]!.rotation : IDENTITY;
      if (vp.tool === "rotate") rotation = this.mode.start.rotation;
    } else {
      if (!targets || !targets.length) return null;
      if (vp.tool === "scale" && !targets.some((t) => t.scale)) return null;
      const n = targets.length;
      position = vp.tool === "translate" ? scale(targets.reduce((a, t) => add(a, t.position), [0, 0, 0] as Vec3), 1 / n) : targets[targets.length - 1]!.position;
      rotation = targets[targets.length - 1]!.rotation;
    }
    return buildGizmo(
      {
        mode: vp.tool,
        space: vp.space,
        position,
        rotation,
        hovered: this.hoverHandle,
        active: this.mode.kind === "gizmo" ? this.mode.start.handle : null,
      },
      this.view(),
    );
  }

  // ---- Camera ----------------------------------------------------------------------------------

  setOrbit(o: Partial<Orbit>, animateMs = 0) {
    const to = { ...this.orbit, ...o, pitch: clampPitch(o.pitch ?? this.orbit.pitch) };
    if (animateMs > 0) this.anim = { from: { ...this.orbit }, to, t0: performance.now(), ms: animateMs };
    else {
      this.anim = null;
      this.orbit = to;
      this.changed();
    }
  }

  look(dir: "+x" | "-x" | "+y" | "-y" | "+z" | "-z" | "persp") {
    if (dir === "persp") {
      this.setOrbit({ ortho: !this.orbit.ortho });
      return;
    }
    const yawPitch: Record<string, [number, number]> = {
      "+x": [90, 0],
      "-x": [-90, 0],
      "+y": [this.orbit.yaw / DEG, 89.5],
      "-y": [this.orbit.yaw / DEG, -89.5],
      "+z": [0, 0],
      "-z": [180, 0],
    };
    const [y, p] = yawPitch[dir]!;
    let yaw = y * DEG;
    // Turn the short way round.
    while (yaw - this.orbit.yaw > Math.PI) yaw -= 2 * Math.PI;
    while (yaw - this.orbit.yaw < -Math.PI) yaw += 2 * Math.PI;
    this.setOrbit({ yaw, pitch: p * DEG }, 260);
  }

  /** Frames the selection (or everything). */
  frameSelection() {
    const w = useWorld.getState();
    const ids = useSelection.getState().ids.length ? useSelection.getState().ids : w.order;
    const pts: Vec3[] = [];
    for (const id of ids) {
      const t = w.entities[id]?.components.Transform as { position?: unknown } | undefined;
      if (t?.position !== undefined) pts.push(asVec3(t.position));
    }
    if (!pts.length) return;
    const c = scale(pts.reduce((a, p) => add(a, p), [0, 0, 0] as Vec3), 1 / pts.length);
    const r = Math.max(1.5, ...pts.map((p) => length(sub(p, c))));
    const dist = Math.max(4, (r * 2.4) / Math.tan((this.orbit.fovY * DEG) / 2) / 2 + r);
    this.setOrbit({ target: c, distance: dist }, 280);
  }

  /** The point on the ground (y = 0) under a pixel, for drops. */
  groundAt(x: number, y: number): Vec3 {
    const view = this.view();
    const ray = view.ray(x, y);
    const t = rayPlane(ray.origin, ray.dir, [0, 0, 0], [0, 1, 0]);
    const p = t !== null && t < 5000 ? add(ray.origin, scale(ray.dir, t)) : this.orbit.target;
    return p.map((v) => Math.round(v * 100) / 100) as Vec3;
  }

  // ---- Input -----------------------------------------------------------------------------------

  pointerDown(e: PointerEvent, x: number, y: number) {
    if (this.mode.kind !== "none") return;
    const pan = e.button === 1 || (e.button === 2 && e.shiftKey) || (e.button === 0 && e.altKey && e.shiftKey);
    if (pan) {
      this.mode = { kind: "pan", x, y };
      return;
    }
    if (e.button === 2 || (e.button === 0 && e.altKey)) {
      this.mode = { kind: "orbit", x, y, moved: false, button: e.button };
      return;
    }
    if (e.button !== 0) return;
    if (this.gizmo) {
      const handle = hitGizmo(this.gizmo, this.view(), x, y);
      if (handle) {
        const targets = this.targets();
        const tool = useViewport.getState().tool;
        const names = targets.length === 1 ? entityName(targets[0]!.id) : `${targets.length} entities`;
        const verb = tool === "translate" ? "Move" : tool === "rotate" ? "Rotate" : "Scale";
        this.mode = {
          kind: "gizmo",
          start: beginDrag(this.gizmo, handle, this.view(), x, y),
          targets,
          edit: new LiveEdit(`${verb} ${names}`),
          snapInvert: e.ctrlKey || e.metaKey,
        };
        return;
      }
    }
    this.mode = { kind: "click", x, y, additive: e.shiftKey || e.metaKey || e.ctrlKey };
  }

  pointerMove(e: PointerEvent, x: number, y: number) {
    const m = this.mode;
    dragState.active = m.kind === "gizmo" || m.kind === "rect";
    switch (m.kind) {
      case "none":
        this.hover(x, y);
        return;
      case "orbit": {
        const dx = x - m.x;
        const dy = y - m.y;
        if (Math.abs(dx) + Math.abs(dy) > 1) m.moved = true;
        this.setOrbit({ yaw: this.orbit.yaw - dx * 0.006, pitch: this.orbit.pitch + dy * 0.006 });
        m.x = x;
        m.y = y;
        return;
      }
      case "pan": {
        const view = this.view();
        const px = view.pixelSize(this.orbit.target);
        const delta = add(scale(view.right, -(x - m.x) * px), scale(view.up, (y - m.y) * px));
        this.setOrbit({ target: add(this.orbit.target, delta) });
        m.x = x;
        m.y = y;
        return;
      }
      case "click":
        if (Math.hypot(x - m.x, y - m.y) > 4) {
          this.mode = { kind: "rect", x0: m.x, y0: m.y, x1: x, y1: y, additive: m.additive };
          this.updateRect();
        }
        return;
      case "rect":
        m.x1 = x;
        m.y1 = y;
        this.updateRect();
        return;
      case "gizmo": {
        const snap = { ...useViewport.getState().snap };
        if (m.snapInvert !== (e.ctrlKey || e.metaKey)) snap.enabled = !snap.enabled;
        const r = dragTo(m.start, this.view(), x, y, snap);
        for (const t of m.targets) {
          if (r.translate) {
            const p = add(t.position, r.translate).map((v) => Math.round(v * 10000) / 10000) as Vec3;
            m.edit.update(t.id, "Transform", { position: t.arrays ? p : { x: p[0], y: p[1], z: p[2] } });
          }
          if (r.rotate) {
            const q = applyRotation(r.rotate, t.rotation).map((v) => Math.round(v * 1e6) / 1e6) as Quat;
            m.edit.update(t.id, "Transform", { rotation: t.arrays ? q : { x: q[0], y: q[1], z: q[2], w: q[3] } });
          }
          if (r.scale && t.scale && t.scaleComponent) {
            const s = t.scale.map((v, i) => Math.round(v * r.scale![i]! * 10000) / 10000) as Vec3;
            m.edit.update(t.id, t.scaleComponent, { scale: t.arrays ? s : { x: s[0], y: s[1], z: s[2] } });
          }
        }
        return;
      }
    }
  }

  async pointerUp(_e: PointerEvent, x: number, y: number): Promise<{ contextMenu: boolean }> {
    const m = this.mode;
    this.mode = { kind: "none" };
    dragState.active = false;
    if (m.kind === "gizmo") {
      await m.edit.commit();
      return { contextMenu: false };
    }
    if (m.kind === "orbit") return { contextMenu: m.button === 2 && !m.moved };
    if (m.kind === "click") {
      const hit = await this.engine?.pick(x, y);
      const sel = useSelection.getState();
      if (hit) {
        if (m.additive) sel.toggle(hit.entity);
        else sel.set([hit.entity]);
      } else if (!m.additive) sel.clear();
      return { contextMenu: false };
    }
    if (m.kind === "rect") {
      this.selectRect(m);
      this.rect = null;
      this.changed();
    }
    return { contextMenu: false };
  }

  /** Escape: cancels a drag (and its live edits). */
  cancel(): boolean {
    const m = this.mode;
    dragState.active = false;
    if (m.kind === "gizmo") {
      this.mode = { kind: "none" };
      void m.edit.cancel();
      return true;
    }
    if (m.kind === "rect") {
      this.mode = { kind: "none" };
      this.rect = null;
      this.changed();
      return true;
    }
    return false;
  }

  wheel(e: WheelEvent) {
    const k = e.ctrlKey ? 0.01 : 0.0012;
    const factor = Math.exp(Math.max(-0.5, Math.min(0.5, e.deltaY * k)));
    this.setOrbit({ distance: Math.max(0.3, Math.min(5000, this.orbit.distance * factor)) });
  }

  private updateRect() {
    const m = this.mode;
    if (m.kind !== "rect") return;
    this.rect = { x: Math.min(m.x0, m.x1), y: Math.min(m.y0, m.y1), w: Math.abs(m.x1 - m.x0), h: Math.abs(m.y1 - m.y0) };
    this.changed();
  }

  private selectRect(m: Extract<Mode, { kind: "rect" }>) {
    const view = this.view();
    const r = { x0: Math.min(m.x0, m.x1), x1: Math.max(m.x0, m.x1), y0: Math.min(m.y0, m.y1), y1: Math.max(m.y0, m.y1) };
    const w = useWorld.getState();
    const ids: EntityId[] = [];
    for (const id of w.order) {
      const t = w.entities[id]?.components.Transform as { position?: unknown } | undefined;
      if (t?.position === undefined) continue;
      const p = view.project(asVec3(t.position));
      if (p.visible && p.x >= r.x0 && p.x <= r.x1 && p.y >= r.y0 && p.y <= r.y1) ids.push(id);
    }
    const sel = useSelection.getState();
    if (m.additive) sel.set([...new Set([...sel.ids, ...ids])]);
    else sel.set(ids);
  }

  private hover(x: number, y: number) {
    if (this.gizmo) {
      const h = hitGizmo(this.gizmo, this.view(), x, y);
      if (h !== this.hoverHandle) this.hoverHandle = h;
      if (h) {
        useSelection.getState().hover(null);
        return;
      }
    } else this.hoverHandle = null;
    const now = performance.now();
    if (now - this.lastHoverPick < 60 || !this.engine) return;
    this.lastHoverPick = now;
    void this.engine.pick(x, y).then((hit) => useSelection.getState().hover(hit?.entity ?? null));
  }

  leave() {
    this.hoverHandle = null;
    useSelection.getState().hover(null);
  }

  get dragging(): boolean {
    return this.mode.kind !== "none";
  }

  get cursor(): string {
    if (this.mode.kind === "orbit") return "grabbing";
    if (this.mode.kind === "pan") return "move";
    if (this.mode.kind === "gizmo" || this.hoverHandle) return "pointer";
    return "default";
  }

  /** The wave clock: host time, advancing locally between status events while playing. */
  static clock(): number {
    const s = useSession.getState();
    const st = s.status;
    if (!st) return 0;
    if (st.mode !== "play" || st.paused) return st.t_s;
    return st.t_s + (performance.now() - s.receivedAt) / 1000;
  }
}
