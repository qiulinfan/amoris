// The mock's scene: the sailing sample (samples/sailing/scene.json) as the prefabs expand it, plus a
// sun, a camera and a small buoy course so the hierarchy has lights, cameras and children.

import { World } from "./world";

export type V3 = [number, number, number];
export type Q4 = [number, number, number, number];

const DEG = Math.PI / 180;

/** Turned `yawDeg` about +y (a boat whose bow is -z heads `-yawDeg`), as `Transform::at_yaw`. */
export function yaw(yawDeg: number): Q4 {
  const a = yawDeg * DEG;
  return [0, Math.sin(a / 2), 0, Math.cos(a / 2)];
}

/** The rotation that points -z from `eye` toward `target` (yaw, then pitch). */
export function lookAt(eye: V3, target: V3): Q4 {
  const f = [target[0] - eye[0], target[1] - eye[1], target[2] - eye[2]];
  const n = Math.hypot(f[0]!, f[1]!, f[2]!) || 1;
  const fx = f[0]! / n;
  const fy = f[1]! / n;
  const fz = f[2]! / n;
  const psi = Math.atan2(-fx, -fz);
  const theta = Math.asin(Math.max(-1, Math.min(1, fy)));
  const sy = Math.sin(psi / 2);
  const cy = Math.cos(psi / 2);
  const sx = Math.sin(theta / 2);
  const cx = Math.cos(theta / 2);
  return [cy * sx, cx * sy, -sy * sx, cy * cx];
}

export function crate(name: string, position: V3, yawDeg: number, value: number) {
  return {
    name,
    components: {
      Transform: { position, rotation: yaw(yawDeg) },
      Velocity: { linear: [0, 0, 0], angular: [0, 0, 0] },
      RigidBody: { kind: "Dynamic", mass: 0, linear_damping: 0.4, angular_damping: 0.8, gravity_scale: 1, ccd: false },
      Collider: { shape: { Cuboid: { half_extents: [0.3, 0.3, 0.3] } }, density: 350, friction: 0.5, restitution: 0.1, sensor: false },
      Cargo: { value },
      Model: { mesh: "models/crate.glb", color: [0.42, 0.25, 0.11, 1], scale: [0.6, 0.6, 0.6], cast_shadows: true, visible: true },
    },
  };
}

export function sailingWorld(): World {
  const w = new World();
  w.spawnRaw("Sea", {
    Sea: {
      level: 0,
      density: 1025,
      waves: [
        { length: 9, height: 0.35, toward_deg: 100, phase: 0 },
        { length: 5.58, height: 0.175, toward_deg: 126, phase: 1.3 },
        { length: 3.6, height: 0.09, toward_deg: 64, phase: 2.1 },
        { length: 2.2, height: 0.04, toward_deg: 150, phase: 0.6 },
      ],
    },
  });
  w.spawnRaw("Breeze", { Wind: { from_deg: 270, speed: 6, gust: 0, gust_period: 20, gust_length: 120 } });
  w.spawnRaw("Sloop", {
    Transform: { position: [0, 0, 0], rotation: yaw(-90) },
    Velocity: { linear: [0, 0, 0], angular: [0, 0, 0] },
    RigidBody: { kind: "Dynamic", mass: 1150, linear_damping: 0.05, angular_damping: 0.6, gravity_scale: 1, ccd: false },
    Collider: { shape: { Cuboid: { half_extents: [1, 0.5, 3] } }, density: null, friction: 0.4, restitution: 0.05, sensor: false },
    Boat: {
      hoist: 1, sheet: 1, rudder: 0, hoist_now: 1, sheet_now: 1, rudder_now: 0, speed: 0, heading_deg: 90, heel_deg: 0,
      afloat: true, aground: false, awa_deg: 180, aws: 6, boom_deg: 0, drive: 0, trim: "Good",
    },
    Sail: { mast: [0, 0.4, -0.6], boom: 2.6, rise: 2.4, area: 9 },
    Crew: { take: null },
    Tally: { taken: 0, worth: 0, total: 0 },
    Log: { distance: 0, top_speed: 0, sail_set: false },
    Model: { mesh: "models/sloop.glb", color: [0.86, 0.88, 0.9, 1], scale: [1, 1, 1], cast_shadows: true, visible: true },
  });
  for (const c of [
    crate("Crate1", [5, 0, 2.2], 10, 1),
    crate("Crate2", [11, 0, -2.4], 35, 2),
    crate("Crate3", [18, 0, 2.6], 60, 1),
    crate("Crate4", [26, 0, -6], 80, 3),
  ]) {
    w.spawnRaw(c.name, c.components);
  }
  w.spawnRaw("Sun", {
    Transform: { position: [-20, 40, 20], rotation: lookAt([-20, 40, 20], [0, 0, 0]) },
    Light: { kind: "Directional", color: [1, 0.95, 0.86], intensity: 100000, range: 0, spot_angle_deg: 45, shadows: true },
  });
  w.spawnRaw("Camera", {
    Transform: { position: [-14, 7, 16], rotation: lookAt([-14, 7, 16], [10, 0, 0]) },
    Camera: { fov_deg: 55, near: 0.1, far: 2000, active: true },
  });
  const course = w.spawnRaw("Course", { Transform: { position: [0, 0, 0], rotation: [0, 0, 0, 1] } });
  const buoy = (name: string, position: V3, color: [number, number, number, number]) =>
    w.spawnRaw(name, {
      Parent: { parent: course.id },
      Transform: { position, rotation: [0, 0, 0, 1] },
      Collider: { shape: { Ball: { radius: 0.45 } }, density: 300, friction: 0.5, restitution: 0.2, sensor: false },
      Model: { mesh: "primitive:sphere", color, scale: [0.9, 0.9, 0.9], cast_shadows: true, visible: true },
    });
  buoy("Buoy A", [14, 0.1, 6], [0.95, 0.32, 0.05, 1]);
  buoy("Buoy B", [32, 0.1, 3], [0.95, 0.32, 0.05, 1]);
  buoy("Finish", [40, 0.1, -2], [0.1, 0.7, 0.25, 1]);
  return w;
}
