// A courtyard at dusk that shows the renderer at once: a procedural sky and a low sun with
// cascaded shadows, volumetric fog the colonnade cuts into shafts, a reflecting pool (screen-space
// reflections), lanterns (clustered point lights, the nearest casting shadows), a spot on the
// crates in a pavilion whose polished floor a reflection probe keeps from mirroring the sky, AgX,
// bloom, ambient occlusion and TAA. The camera circles slowly; the orb bobs over the pool; a
// timeline (timelines/dusk.json, on the Dusk entity) sinks and reddens the sun and brings up the
// pavilion lamp over 24 seconds, then back. Three visitors (the built-in humanoid, walked by
// Animator.locomotion) stroll a path round the pool, and fireflies (particles on the GPU, stirred
// by turbulence) drift over the courtyard.
//
// Open it in the editor (`pocket editor showcase`) or run it (`pocket run showcase`).
import { expose, onStart, onTick, world } from "pocket";

const radius = 20;
const height = 6.5;
let angle = 0.35;
let camera = 0;
let orb = 0;

// A rotation that turns -Z to the given yaw (degrees about Y) and pitch (about X, down negative).
function lookRotation(yaw: number, pitch: number) {
    const y = yaw / 2, p = pitch / 2;
    const cy = Math.cos(y), sy = Math.sin(y), cp = Math.cos(p), sp = Math.sin(p);
    return { x: cy * sp, y: sy * cp, z: -sy * sp, w: cy * cp };
}

function placeCamera() {
    const x = Math.sin(angle) * radius, z = Math.cos(angle) * radius;
    const pitch = -Math.atan2(height - 1.5, radius);
    world.set(camera, "Transform", { position: { x, y: height, z }, rotation: lookRotation(angle, pitch) });
}

onStart(() => {
    camera = world.find("Camera") ?? 0;
    orb = world.find("Orb") ?? 0;
    if (camera) placeCamera();
});

onTick(({ dt, time }) => {
    angle += dt * 0.04;
    if (camera) placeCamera();
    if (orb) world.set(orb, "Transform", { position: { x: 0, y: 2.2 + Math.sin(time * 0.8) * 0.3, z: 1 } });
});

expose("camera.angle", () => Number(angle.toFixed(4)));
