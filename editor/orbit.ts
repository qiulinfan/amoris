// Orbit camera for the scene pane: yaw/pitch/distance around a target, written back to the
// Camera entity's Transform. The engine's camera looks down its local -Z.
import { render, world } from "pocket";

export interface Orbit {
    yaw: number;
    pitch: number;
    distance: number;
    target: { x: number; y: number; z: number };
}

export function orbitFromCamera(): Orbit | undefined {
    const cam = render.stats().camera;
    if (!cam) return undefined;
    const t = world.get(cam, "Transform");
    if (!t) return undefined;
    const p = t.position;
    const distance = Math.max(0.5, Math.hypot(p.x, p.y, p.z));
    return { yaw: Math.atan2(p.x, p.z), pitch: Math.asin(Math.max(-1, Math.min(1, p.y / distance))), distance, target: { x: 0, y: 0, z: 0 } };
}

export function applyOrbit(o: Orbit): void {
    const cam = render.stats().camera;
    if (!cam) return;
    const cp = Math.cos(o.pitch);
    const pos = { x: o.target.x + o.distance * Math.sin(o.yaw) * cp, y: o.target.y + o.distance * Math.sin(o.pitch), z: o.target.z + o.distance * Math.cos(o.yaw) * cp };
    // Basis: forward f toward the target, right r, up u; the camera's local +Z is -f.
    let fx = o.target.x - pos.x, fy = o.target.y - pos.y, fz = o.target.z - pos.z;
    const fl = Math.hypot(fx, fy, fz) || 1;
    fx /= fl; fy /= fl; fz /= fl;
    let rx = fz * 1 - fy * 0, ry = fx * 0 - fz * 0, rz = fy * 0 - fx * 1;  // f x up(0,1,0)
    rx = fz; ry = 0; rz = -fx;
    const rl = Math.hypot(rx, ry, rz) || 1;
    rx /= rl; ry /= rl; rz /= rl;
    const ux = ry * fz - rz * fy, uy = rz * fx - rx * fz, uz = rx * fy - ry * fx;  // r x f
    // Rotation matrix columns: right, up, back (-f). Convert to a quaternion.
    const m00 = rx, m01 = ux, m02 = -fx;
    const m10 = ry, m11 = uy, m12 = -fy;
    const m20 = rz, m21 = uz, m22 = -fz;
    const trace = m00 + m11 + m22;
    let x: number, y: number, z: number, w: number;
    if (trace > 0) {
        const s = Math.sqrt(trace + 1) * 2;
        w = 0.25 * s; x = (m21 - m12) / s; y = (m02 - m20) / s; z = (m10 - m01) / s;
    } else if (m00 > m11 && m00 > m22) {
        const s = Math.sqrt(1 + m00 - m11 - m22) * 2;
        w = (m21 - m12) / s; x = 0.25 * s; y = (m01 + m10) / s; z = (m02 + m20) / s;
    } else if (m11 > m22) {
        const s = Math.sqrt(1 + m11 - m00 - m22) * 2;
        w = (m02 - m20) / s; x = (m01 + m10) / s; y = 0.25 * s; z = (m12 + m21) / s;
    } else {
        const s = Math.sqrt(1 + m22 - m00 - m11) * 2;
        w = (m10 - m01) / s; x = (m02 + m20) / s; y = (m12 + m21) / s; z = 0.25 * s;
    }
    world.set(cam, "Transform", { position: pos, rotation: { x, y, z, w } });
}
