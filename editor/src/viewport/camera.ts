// The editor's camera: an orbit around a target (yaw, pitch, distance) that the editor owns and
// hands to the renderer as plain parameters (engine.ts `Camera`). `View` does the same projection
// the renderer does, so hit tests on gizmo handles and drops on the ground plane agree with what is
// drawn, whichever renderer draws it.

import { add, cross, DEG, dot, normalize, scale, sub, type Vec3 } from "./math";

export interface Orbit {
  target: Vec3;
  /** Radians about +y; 0 puts the eye on +z looking toward -z. */
  yaw: number;
  /** Radians above the horizon. */
  pitch: number;
  distance: number;
  fovY: number;
  ortho: boolean;
}

/** What `Viewport.setCamera` receives. */
export interface Camera {
  position: Vec3;
  target: Vec3;
  up: Vec3;
  fov_y_deg: number;
  near: number;
  far: number;
  projection: "perspective" | "orthographic";
  /** World units from the bottom to the top of the view, for orthographic projection. */
  ortho_height: number;
}

export const DEFAULT_ORBIT: Orbit = { target: [8, 0, 0], yaw: -38 * DEG, pitch: 24 * DEG, distance: 30, fovY: 50, ortho: false };

const MAX_PITCH = 89.5 * DEG;

export function clampPitch(p: number): number {
  return Math.max(-MAX_PITCH, Math.min(MAX_PITCH, p));
}

export function orbitEye(o: Orbit): Vec3 {
  const cp = Math.cos(o.pitch);
  return add(o.target, scale([cp * Math.sin(o.yaw), Math.sin(o.pitch), cp * Math.cos(o.yaw)], o.distance));
}

export function cameraOf(o: Orbit): Camera {
  return {
    position: orbitEye(o),
    target: o.target,
    up: [0, 1, 0],
    fov_y_deg: o.fovY,
    near: Math.max(0.02, o.distance * 0.002),
    far: Math.max(2000, o.distance * 50),
    projection: o.ortho ? "orthographic" : "perspective",
    ortho_height: 2 * o.distance * Math.tan((o.fovY * DEG) / 2),
  };
}

export interface Projected {
  x: number;
  y: number;
  /** Distance along the view direction. */
  depth: number;
  visible: boolean;
}

export class View {
  readonly right: Vec3;
  readonly up: Vec3;
  readonly forward: Vec3;
  readonly tanHalf: number;
  readonly aspect: number;

  constructor(
    readonly camera: Camera,
    readonly width: number,
    readonly height: number,
  ) {
    this.forward = normalize(sub(camera.target, camera.position));
    let r = cross(this.forward, camera.up);
    if (Math.hypot(...r) < 1e-6) r = cross(this.forward, [0, 0, -1]);
    this.right = normalize(r);
    this.up = cross(this.right, this.forward);
    this.tanHalf = Math.tan((camera.fov_y_deg * DEG) / 2);
    this.aspect = width / Math.max(1, height);
  }

  get ortho(): boolean {
    return this.camera.projection === "orthographic";
  }

  /** World point to camera space: [right, up, forward]. */
  toView(p: Vec3): Vec3 {
    const d = sub(p, this.camera.position);
    return [dot(d, this.right), dot(d, this.up), dot(d, this.forward)];
  }

  /** Camera space to pixels. */
  viewToScreen(v: Vec3): { x: number; y: number } {
    let sx: number;
    let sy: number;
    if (this.ortho) {
      const h = this.camera.ortho_height / 2;
      sx = v[0] / (h * this.aspect);
      sy = v[1] / h;
    } else {
      const z = Math.max(1e-6, v[2]);
      sx = v[0] / (z * this.tanHalf * this.aspect);
      sy = v[1] / (z * this.tanHalf);
    }
    return { x: ((sx + 1) / 2) * this.width, y: ((1 - sy) / 2) * this.height };
  }

  project(p: Vec3): Projected {
    const v = this.toView(p);
    const s = this.viewToScreen(v);
    return { x: s.x, y: s.y, depth: v[2], visible: this.ortho || v[2] > this.camera.near };
  }

  /** The ray through a pixel. */
  ray(x: number, y: number): { origin: Vec3; dir: Vec3 } {
    const nx = (x / this.width) * 2 - 1;
    const ny = 1 - (y / this.height) * 2;
    if (this.ortho) {
      const h = this.camera.ortho_height / 2;
      const origin = add(this.camera.position, add(scale(this.right, nx * h * this.aspect), scale(this.up, ny * h)));
      return { origin, dir: this.forward };
    }
    const dir = normalize(add(this.forward, add(scale(this.right, nx * this.tanHalf * this.aspect), scale(this.up, ny * this.tanHalf))));
    return { origin: this.camera.position, dir };
  }

  /** World units per pixel at a point (for constant-size gizmos). */
  pixelSize(p: Vec3): number {
    if (this.ortho) return this.camera.ortho_height / this.height;
    const depth = Math.max(this.camera.near, this.toView(p)[2]);
    return (2 * depth * this.tanHalf) / this.height;
  }
}
