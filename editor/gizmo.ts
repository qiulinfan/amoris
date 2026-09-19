// Translate gizmo math on top of `render.project`: where the handles go on screen, and how far a
// screen-space drag moves the selection along a world axis.
import { render, world } from "pocket";
import type { Quat, Vec3 } from "pocket";

export type Axis = "x" | "y" | "z" | "plane" | "rotate" | "scale";

export interface GizmoLayout {
    /** Pixel positions of the entity origin and the three axis tips. */
    center: { x: number; y: number };
    tips: { x: { x: number; y: number }; y: { x: number; y: number }; z: { x: number; y: number } };
    /** Axis length in world units. */
    length: number;
}

const AXES: Record<"x" | "y" | "z", Vec3> = { x: { x: 1, y: 0, z: 0 }, y: { x: 0, y: 1, z: 0 }, z: { x: 0, y: 0, z: 1 } };

function projectPoint(p: Vec3): { x: number; y: number } | undefined {
    const r = render.project(p);
    return r.visible && r.x !== undefined && r.y !== undefined ? { x: r.x, y: r.y } : undefined;
}

/** Handle positions in pixels for an entity, or undefined when it is behind the camera. */
export function layoutFor(entity: number): GizmoLayout | undefined {
    const wt = world.get(entity, "WorldTransform");
    if (!wt) return undefined;
    const cam = render.stats().camera;
    const camPos = cam !== undefined ? world.get(cam, "WorldTransform")?.position : undefined;
    const dist = camPos ? Math.hypot(camPos.x - wt.position.x, camPos.y - wt.position.y, camPos.z - wt.position.z) : 10;
    const length = Math.max(0.25, dist * 0.12);
    const center = projectPoint(wt.position);
    if (!center) return undefined;
    const tip = (a: Vec3) => projectPoint({ x: wt.position.x + a.x * length, y: wt.position.y + a.y * length, z: wt.position.z + a.z * length });
    const tx = tip(AXES.x), ty = tip(AXES.y), tz = tip(AXES.z);
    if (!tx || !ty || !tz) return undefined;
    return { center, tips: { x: tx, y: ty, z: tz }, length };
}

/**
 * World displacement for a pixel drag along one axis handle: the drag is projected onto the
 * axis's screen direction, so pulling the X handle right moves +X however the camera is turned.
 */
export function axisDelta(layout: GizmoLayout, axis: "x" | "y" | "z", dxPixels: number, dyPixels: number): Vec3 {
    const tip = layout.tips[axis];
    const sx = tip.x - layout.center.x, sy = tip.y - layout.center.y;
    const len2 = sx * sx + sy * sy;
    if (len2 < 1e-6) return { x: 0, y: 0, z: 0 };
    const t = (dxPixels * sx + dyPixels * sy) / len2;   // fraction of one axis length
    const a = AXES[axis];
    return { x: a.x * t * layout.length, y: a.y * t * layout.length, z: a.z * t * layout.length };
}

/** Camera-plane drag: move along the camera's right and up vectors so the entity follows the mouse. */
export function planeDelta(entity: number, dxPixels: number, dyPixels: number): Vec3 {
    const cam = render.stats().camera;
    const wt = world.get(entity, "WorldTransform");
    const ct = cam !== undefined ? world.get(cam, "WorldTransform") : undefined;
    if (!wt || !ct) return { x: 0, y: 0, z: 0 };
    const right = rotate(ct.rotation, { x: 1, y: 0, z: 0 });
    const up = rotate(ct.rotation, { x: 0, y: 1, z: 0 });
    const c = projectPoint(wt.position);
    const r = projectPoint({ x: wt.position.x + right.x, y: wt.position.y + right.y, z: wt.position.z + right.z });
    const u = projectPoint({ x: wt.position.x + up.x, y: wt.position.y + up.y, z: wt.position.z + up.z });
    if (!c || !r || !u) return { x: 0, y: 0, z: 0 };
    // Pixels per world unit along each screen direction; solve the 2x2 system for the world delta.
    const a = r.x - c.x, b = u.x - c.x, cc = r.y - c.y, d = u.y - c.y;
    const det = a * d - b * cc;
    if (Math.abs(det) < 1e-6) return { x: 0, y: 0, z: 0 };
    const kr = (dxPixels * d - b * dyPixels) / det;
    const ku = (a * dyPixels - cc * dxPixels) / det;
    return { x: right.x * kr + up.x * ku, y: right.y * kr + up.y * ku, z: right.z * kr + up.z * ku };
}

/** q1 * q2 (apply q2, then q1). */
export function multiplyQuat(a: Quat, b: Quat): Quat {
    return {
        x: a.w * b.x + a.x * b.w + a.y * b.z - a.z * b.y,
        y: a.w * b.y - a.x * b.z + a.y * b.w + a.z * b.x,
        z: a.w * b.z + a.x * b.y - a.y * b.x + a.z * b.w,
        w: a.w * b.w - a.x * b.x - a.y * b.y - a.z * b.z,
    };
}

/** Rotation of `radians` around the world Y axis. */
export function yawQuat(radians: number): Quat {
    return { x: 0, y: Math.sin(radians / 2), z: 0, w: Math.cos(radians / 2) };
}

function rotate(q: { x: number; y: number; z: number; w: number }, v: Vec3): Vec3 {
    // v' = v + 2w(q x v) + 2(q x (q x v))
    const cx = q.y * v.z - q.z * v.y, cy = q.z * v.x - q.x * v.z, cz = q.x * v.y - q.y * v.x;
    const dx = q.y * cz - q.z * cy, dy = q.z * cx - q.x * cz, dz = q.x * cy - q.y * cx;
    return { x: v.x + 2 * (q.w * cx + dx), y: v.y + 2 * (q.w * cy + dy), z: v.z + 2 * (q.w * cz + dz) };
}
