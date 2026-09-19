// Thousands of entities moved every tick. Two paths do the same work on alternating ticks:
// `world.pack` / `world.unpack` (typed arrays shared with the engine, two commands per tick) and
// per-entity `world.get` / `world.set` (two JSON round trips per entity). The report exposes the
// average milliseconds of each path, so the difference is a number, not a claim. The timings
// are wall clock and therefore not part of what makes a run reproducible; everything else is.
//   pocket run swarm -- --headless --frames 120 --json
import { expose, log, onStart, onTick, runtime, setClearColor, world } from "pocket";

const COUNT = 3000;
let ids: number[] = [];
let time = 0;
let packMs = 0, packTicks = 0;
let jsonMs = 0, jsonTicks = 0;

onStart(() => {
    setClearColor(0.05, 0.05, 0.08, 1);
    const side = Math.ceil(Math.sqrt(COUNT));
    for (let i = 0; i < COUNT; i++) {
        const x = (i % side) - side / 2;
        const z = Math.floor(i / side) - side / 2;
        ids.push(world.spawn(`b${i}`, { components: { Transform: { position: { x: x * 0.6, y: 0, z: z * 0.6 }, scale: { x: 0.25, y: 0.25, z: 0.25 } }, MeshRenderer: { mesh: "cube", color: { r: 0.3 + (i % 7) / 10, g: 0.5, b: 0.9 - (i % 5) / 10, a: 1 } } } }));
    }
    world.spawn("Sun", { components: { Transform: { rotation: { x: -0.4, y: 0.2, z: 0.1, w: 0.89 } }, Light: { kind: 0, intensity: 1.1 } } });
    world.spawn("Camera", { components: { Transform: { position: { x: 0, y: 22, z: 26 }, rotation: { x: -0.35, y: 0, z: 0, w: 0.94 } }, Camera: { far: 200 } } });
    log("swarm ready", { count: COUNT, headless: runtime().headless });
});

onTick((t) => {
    time += t.dt;
    const start = performance.now();
    const usePack = t.tick % 2 === 0;
    if (usePack) {
        const p = world.pack("Transform", ["position"], { name: "b*" });
        const d = p.data;
        for (let i = 0; i < p.count; i++) {
            const o = i * p.stride;
            d[o + 1] = Math.sin(time * 2 + d[o] * 0.7 + d[o + 2] * 0.5) * 1.5;
        }
        world.unpack();
    } else {
        for (const id of ids) {
            const tr = world.get(id, "Transform")!;
            world.set(id, "Transform", { position: { y: Math.sin(time * 2 + tr.position.x * 0.7 + tr.position.z * 0.5) * 1.5 } });
        }
    }
    const ms = performance.now() - start;
    if (usePack) { packMs += ms; packTicks++; } else { jsonMs += ms; jsonTicks++; }
});

expose("count", () => COUNT);
expose("pack.ms.avg", () => Number((packTicks ? packMs / packTicks : 0).toFixed(3)));
expose("json.ms.avg", () => Number((jsonTicks ? jsonMs / jsonTicks : 0).toFixed(3)));
expose("speedup", () => Number((packTicks && jsonTicks && packMs > 0 ? (jsonMs / jsonTicks) / (packMs / packTicks) : 0).toFixed(1)));
