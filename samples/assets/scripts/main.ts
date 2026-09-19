// Meshes and textures from the project's assets folder: a glTF crate with two baked nodes and a
// checker texture, a pyramid from a .gltf with an embedded buffer, a skinned arm waving through
// its glTF animation, a textured ground, and a deliberately missing asset (drawn as a magenta
// cube, reported in render.stats).
import { animation, command, expose, log, onStart, onTick, world } from "pocket";

interface AssetFile { path: string; kind: string; bytes: number; loaded: boolean }

let crate = 0;
let yaw = 0;

onStart(() => {
    const files = command<AssetFile[]>("assets.list");
    log("assets in the project", { files: files.map((f) => `${f.path} (${f.kind}, ${f.bytes} B)`) });
    const crateInfo = command<{ vertices: number; triangles: number; materials: unknown[]; nodes: number }>("assets.describe", { path: "assets/crate.glb" });
    log("crate.glb", { vertices: crateInfo.vertices, triangles: crateInfo.triangles, nodes: crateInfo.nodes });
    crate = world.find("Crate") ?? 0;
    const arm = world.find("Arm");
    if (arm !== undefined) log("arm.glb", animation.clips(arm));
});

let armClip = "wave";

onTick((t) => {
    yaw += t.dt * 0.6;
    if (crate) world.set(crate, "Transform", { rotation: { x: 0, y: Math.sin(yaw / 2), z: 0, w: Math.cos(yaw / 2) } });
    // Every four seconds the arm cross-fades between its two clips over half a second.
    if (t.tick > 0 && t.tick % 240 === 0) {
        const arm = world.find("Arm");
        if (arm !== undefined) {
            armClip = armClip === "wave" ? "nod" : "wave";
            animation.play(arm, armClip, { fade: 0.5 });
        }
    }
});

expose("yaw", () => Number(yaw.toFixed(3)));
expose("arm.clip", () => armClip);
expose("arm.time", () => Number((world.get(world.find("Arm") ?? 0, "Animator")?.time ?? 0).toFixed(3)));
// The Walker paces: its walk clip carries it along its own +Z through root motion, and every
// three seconds the script turns it around (docs/design/animation.md, Root motion).
let walkerTurns = 0;
onTick(({ tick }) => {
    const walker = world.find("Walker");
    if (walker === undefined || tick === 0 || tick % 180 !== 0) return;
    walkerTurns++;
    const half = (walkerTurns * Math.PI) / 2;
    world.set(walker, "Transform", { rotation: { x: 0, y: Math.sin(half), z: 0, w: Math.cos(half) } });
});
expose("walker.z", () => Number((world.get(world.find("Walker") ?? 0, "Transform")?.position.z ?? 0).toFixed(3)));
expose("walker.turns", () => walkerTurns);
expose("pulse.bulge", () => { const p = world.find("Pulse"); return p === undefined ? 0 : Number((animation.pose(p).weights?.[0]?.weight ?? 0).toFixed(3)); });
expose("missing", () => (command<{ assets?: { missing?: string[] } }>("render.stats").assets?.missing ?? []).length);
