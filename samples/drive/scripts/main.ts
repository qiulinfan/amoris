// A car on raycast wheels (the Vehicle component, docs/design/physics.md, Vehicles) over rolling
// grassland (a Terrain with scattered tufts): W and S or the triggers drive, A and D or the stick
// steer, Space or A brakes. Four gates wait in order; the camera chases the car.
import { events, expose, input, onStart, onTick, terrain, world } from "pocket";

let car = 0;
let camera = 0;
let gates = 0;
let seen = 0;
let yaw = 0;

// A rotation that turns -Z to the given yaw (radians about Y) and pitch (about X, down negative).
function lookRotation(y: number, pitch: number) {
    const h = y / 2, p = pitch / 2;
    const cy = Math.cos(h), sy = Math.sin(h), cp = Math.cos(p), sp = Math.sin(p);
    return { x: cy * sp, y: sy * cp, z: -sy * sp, w: cy * cp };
}

function heading(): number {
    const q = world.get(car, "Transform")!.rotation;
    // The car's forward (-Z) turned by its rotation, flattened: its heading about Y.
    const fx = -(2 * (q.x * q.z + q.w * q.y)), fz = -(1 - 2 * (q.x * q.x + q.y * q.y));
    return Math.atan2(-fx, -fz);
}

onStart(() => {
    car = world.find("Car") ?? 0;
    camera = world.find("Camera") ?? 0;
    seen = events.lastSeq();
    if (car) yaw = heading();
    // Each gate stands on the ground, facing the way from the one before it.
    let from = { x: 0, z: 0 };
    for (let i = 1; world.find(`Gate${i}`); i++) {
        const gate = world.find(`Gate${i}`)!;
        const p = world.get(gate, "Transform")!.position;
        const face = Math.atan2(-(p.x - from.x), -(p.z - from.z));
        world.set(gate, "Transform", { position: { x: p.x, y: terrain.height(p.x, p.z).height, z: p.z }, rotation: { x: 0, y: Math.sin(face / 2), z: 0, w: Math.cos(face / 2) } });
        from = { x: p.x, z: p.z };
    }
});

onTick(({ dt }) => {
    if (!car) return;
    world.set(car, "Vehicle", { throttle: input.axis("throttle"), steer: input.axis("steer"), brake: input.down("brake") ? 1 : 0 });
    // Gates count in order: the next one driven through.
    for (const e of events.since(seen)) {
        seen = e.seq;
        if (e.type !== "trigger.enter") continue;
        const d = e.data as { a: string; b: string };
        const gate = [d.a, d.b].find((p) => p.startsWith("/Gates/Gate"));
        if (gate && [d.a, d.b].includes("/Car") && gate === `/Gates/Gate${gates + 1}`) {
            gates++;
            events.emit("gate.passed", { gate: gates }, { subject: car });
        }
    }
    // The camera behind the car along its heading, easing round with it.
    if (camera) {
        const p = world.get(car, "Transform")!.position;
        const h = heading();
        let turn = h - yaw;
        while (turn > Math.PI) turn -= 2 * Math.PI;
        while (turn < -Math.PI) turn += 2 * Math.PI;
        yaw += turn * Math.min(1, dt * 3);
        const back = 11, up = 4.5;
        const at = { x: p.x + Math.sin(yaw) * back, y: p.y + up, z: p.z + Math.cos(yaw) * back };
        world.set(camera, "Transform", { position: at, rotation: lookRotation(yaw, -Math.atan2(up - 1, back)) });
    }
});

expose("gates", () => gates);
expose("gate.x", () => { const g = world.find(`Gate${gates + 1}`); return g ? world.get(g, "Transform")!.position.x : 0; });
expose("gate.z", () => { const g = world.find(`Gate${gates + 1}`); return g ? world.get(g, "Transform")!.position.z : 0; });
expose("speed", () => Number(world.get(car, "Vehicle")!.speed.toFixed(3)));
expose("wheels", () => world.get(car, "Vehicle")!.grounded);
expose("heading", () => Number(heading().toFixed(4)));
expose("car.x", () => Number(world.get(car, "Transform")!.position.x.toFixed(3)));
expose("car.y", () => Number(world.get(car, "Transform")!.position.y.toFixed(3)));
expose("car.z", () => Number(world.get(car, "Transform")!.position.z.toFixed(3)));
expose("upright", () => {
    const q = world.get(car, "Transform")!.rotation;
    return Number((1 - 2 * (q.x * q.x + q.z * q.z)).toFixed(4));   // the car's up, its y: 1 level, 0 on its side
});
