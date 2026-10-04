// Small vector and quaternion math for the viewport and the inspector. Conventions match the engine
// (crates/pocket-physics geom): right-handed, +y up, quaternions `[x, y, z, w]`. Euler angles shown
// in the inspector are degrees in YXZ order (yaw about Y, then pitch about X, then roll about Z),
// the order editors for y-up engines use.

import type { Quat, Vec3 } from "../host/protocol";

export type { Quat, Vec3 };

export const DEG = Math.PI / 180;

export const add = (a: Vec3, b: Vec3): Vec3 => [a[0] + b[0], a[1] + b[1], a[2] + b[2]];
export const sub = (a: Vec3, b: Vec3): Vec3 => [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
export const scale = (a: Vec3, s: number): Vec3 => [a[0] * s, a[1] * s, a[2] * s];
export const mul3 = (a: Vec3, b: Vec3): Vec3 => [a[0] * b[0], a[1] * b[1], a[2] * b[2]];
export const dot = (a: Vec3, b: Vec3): number => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
export const cross = (a: Vec3, b: Vec3): Vec3 => [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
export const length = (a: Vec3): number => Math.hypot(a[0], a[1], a[2]);
export const normalize = (a: Vec3): Vec3 => {
  const l = length(a);
  return l > 1e-12 ? scale(a, 1 / l) : [0, 0, 0];
};
export const lerp3 = (a: Vec3, b: Vec3, t: number): Vec3 => [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];

export const IDENTITY: Quat = [0, 0, 0, 1];

export function qmul(a: Quat, b: Quat): Quat {
  const [ax, ay, az, aw] = a;
  const [bx, by, bz, bw] = b;
  return [
    aw * bx + ax * bw + ay * bz - az * by,
    aw * by - ax * bz + ay * bw + az * bx,
    aw * bz + ax * by - ay * bx + az * bw,
    aw * bw - ax * bx - ay * by - az * bz,
  ];
}

export function qnormalize(q: Quat): Quat {
  const l = Math.hypot(q[0], q[1], q[2], q[3]);
  return l > 1e-12 ? [q[0] / l, q[1] / l, q[2] / l, q[3] / l] : IDENTITY;
}

export function qconj(q: Quat): Quat {
  return [-q[0], -q[1], -q[2], q[3]];
}

export function qaxis(axis: Vec3, rad: number): Quat {
  const s = Math.sin(rad / 2);
  const n = normalize(axis);
  return [n[0] * s, n[1] * s, n[2] * s, Math.cos(rad / 2)];
}

export function rotate(q: Quat, v: Vec3): Vec3 {
  const u: Vec3 = [q[0], q[1], q[2]];
  const w = q[3];
  const t = scale(cross(u, v), 2);
  return add(add(v, scale(t, w)), cross(u, t));
}

export function qslerp(a: Quat, b: Quat, t: number): Quat {
  let d = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
  let bb = b;
  if (d < 0) {
    d = -d;
    bb = [-b[0], -b[1], -b[2], -b[3]];
  }
  if (d > 0.9995) return qnormalize([a[0] + (bb[0] - a[0]) * t, a[1] + (bb[1] - a[1]) * t, a[2] + (bb[2] - a[2]) * t, a[3] + (bb[3] - a[3]) * t]);
  const th = Math.acos(d);
  const s = Math.sin(th);
  const wa = Math.sin((1 - t) * th) / s;
  const wb = Math.sin(t * th) / s;
  return [a[0] * wa + bb[0] * wb, a[1] * wa + bb[1] * wb, a[2] * wa + bb[2] * wb, a[3] * wa + bb[3] * wb];
}

/** Quaternion from Euler degrees [x, y, z], applied Y then X then Z (intrinsic YXZ). */
export function quatFromEuler(deg: Vec3): Quat {
  const [x, y, z] = deg.map((d) => (d * DEG) / 2) as Vec3;
  const c1 = Math.cos(x);
  const c2 = Math.cos(y);
  const c3 = Math.cos(z);
  const s1 = Math.sin(x);
  const s2 = Math.sin(y);
  const s3 = Math.sin(z);
  return [s1 * c2 * c3 + c1 * s2 * s3, c1 * s2 * c3 - s1 * c2 * s3, c1 * c2 * s3 - s1 * s2 * c3, c1 * c2 * c3 + s1 * s2 * s3];
}

/** Euler degrees [x, y, z] (YXZ) of a quaternion. */
export function eulerFromQuat(q: Quat): Vec3 {
  const [x, y, z, w] = qnormalize(q);
  const x2 = x + x;
  const y2 = y + y;
  const z2 = z + z;
  const xx = x * x2;
  const xy = x * y2;
  const xz = x * z2;
  const yy = y * y2;
  const yz = y * z2;
  const zz = z * z2;
  const wx = w * x2;
  const wy = w * y2;
  const wz = w * z2;
  const m11 = 1 - (yy + zz);
  const m13 = xz + wy;
  const m21 = xy + wz;
  const m22 = 1 - (xx + zz);
  const m23 = yz - wx;
  const m31 = xz - wy;
  const m33 = 1 - (xx + yy);
  const ex = Math.asin(-Math.max(-1, Math.min(1, m23)));
  let ey: number;
  let ez: number;
  if (Math.abs(m23) < 0.9999999) {
    ey = Math.atan2(m13, m33);
    ez = Math.atan2(m21, m22);
  } else {
    ey = Math.atan2(-m31, m11);
    ez = 0;
  }
  const clean = (r: number) => {
    const d = r / DEG;
    const rounded = Math.round(d * 1000) / 1000;
    return Object.is(rounded, -0) ? 0 : rounded;
  };
  return [clean(ex), clean(ey), clean(ez)];
}

export function quatClose(a: Quat, b: Quat, eps = 1e-5): boolean {
  const d = Math.abs(a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3]);
  return 1 - d < eps;
}

/** Ray-plane intersection: the distance along the ray, or null when parallel or behind. */
export function rayPlane(origin: Vec3, dir: Vec3, point: Vec3, normal: Vec3): number | null {
  const d = dot(dir, normal);
  if (Math.abs(d) < 1e-9) return null;
  const t = dot(sub(point, origin), normal) / d;
  return t >= 0 ? t : null;
}

/** The parameter along line (p, u) of its closest point to the ray (o, d). */
export function closestOnLine(p: Vec3, u: Vec3, o: Vec3, d: Vec3): number {
  const w = sub(p, o);
  const a = dot(u, u);
  const b = dot(u, d);
  const c = dot(d, d);
  const dd = dot(u, w);
  const e = dot(d, w);
  const den = a * c - b * b;
  if (Math.abs(den) < 1e-9) return 0;
  return (b * e - c * dd) / den;
}

export function snapTo(v: number, step: number): number {
  return step > 0 ? Math.round(v / step) * step : v;
}

export function asVec3(v: unknown, fallback: Vec3 = [0, 0, 0]): Vec3 {
  if (Array.isArray(v) && v.length >= 3) return [Number(v[0]) || 0, Number(v[1]) || 0, Number(v[2]) || 0];
  if (v && typeof v === "object" && "x" in v) {
    const o = v as Record<string, number>;
    return [o.x ?? 0, o.y ?? 0, o.z ?? 0];
  }
  return fallback;
}

export function asQuat(v: unknown): Quat {
  if (Array.isArray(v) && v.length >= 4) return [Number(v[0]) || 0, Number(v[1]) || 0, Number(v[2]) || 0, Number(v[3]) || 1];
  if (v && typeof v === "object" && "w" in v) {
    const o = v as Record<string, number>;
    return [o.x ?? 0, o.y ?? 0, o.z ?? 0, o.w ?? 1];
  }
  return IDENTITY;
}
