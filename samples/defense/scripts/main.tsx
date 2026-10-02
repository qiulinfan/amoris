// Tower defense (docs/design/paths.md; docs/design/combat.md): raiders walk the road (a Path, each a
// PathFollower) toward the keep; towers built beside it shoot the nearest one in range. The mouse
// points at a cell of the meadow, or the arrow keys and WASD step the cursor; the left button,
// Space or A builds a tower there for 50 gold, never on the road nor on another tower. A raider
// down pays 10 gold; one that reaches the keep costs a life; at none the keep has fallen. Each wave
// is two raiders more than the last, hardier and quicker.
//   pocket run defense
//   pocket scenario defense
import { Label, audio, combat, events, expose, input, mount, onStart, onTick, particles, render, signal, world } from "pocket";
import type { Entity, Vec3 } from "pocket";

const CELL = 2;                 // the meadow's cells, two units across
const COLS = 8, ROWS = 6;       // cells from the middle each way: x within ±16, z within ±12
const COST = 50;
const RANGE = 7;
const RELOAD = 0.6;             // seconds between a tower's shots
const ROAD: Array<[number, number]> = [[-17, -10], [-6, -10], [-6, 4], [6, 4], [6, -6], [17, -6]];

interface Tower { id: Entity; x: number; z: number; wait: number }
let towers: Tower[] = [];
let raiders: Entity[] = [];
let queued = 0;                 // raiders of this wave still to come
let gap = 0;                    // seconds to the next one
let rest = 2;                   // seconds to the next wave
let made = 0;
let seen = 0;
let cell = { c: 0, r: 0 };
let pointer = { x: -1, y: -1 };
const gold = signal(150);
const lives = signal(10);
const wave = signal(0);
const kills = signal(0);
const fallen = signal(false);

// Whether a cell's square touches the road (a band two units wide along each straight piece).
function onRoad(x: number, z: number): boolean {
    for (let i = 0; i + 1 < ROAD.length; i++) {
        const [x0, z0] = ROAD[i], [x1, z1] = ROAD[i + 1];
        if (x >= Math.min(x0, x1) - 1.9 && x <= Math.max(x0, x1) + 1.9 && z >= Math.min(z0, z1) - 1.9 && z <= Math.max(z0, z1) + 1.9) return true;
    }
    return false;
}
const centre = (c: { c: number; r: number }) => ({ x: c.c * CELL, z: c.r * CELL });
function buildable(c: { c: number; r: number }): boolean {
    const p = centre(c);
    return !onRoad(p.x, p.z) && !towers.some((t) => t.x === p.x && t.z === p.z);
}

function build() {
    const p = centre(cell);
    if (!buildable(cell) || gold() < COST) {
        events.emit("build.refused", { x: p.x, z: p.z, gold: gold(), why: gold() < COST ? "gold" : "place" });
        audio.play("sfx:blip?volume=0.3");
        return;
    }
    gold.set(gold() - COST);
    const id = world.spawn(`Tower${towers.length}`, { components: {
        Transform: { position: { x: p.x, y: 0, z: p.z } },
        MeshRenderer: { mesh: "tower" },
        RigidBody: { kind: "static" },
        Collider: { shape: "capsule", size: { x: 0.7, y: 1, z: 0.7 }, offset: { x: 0, y: 1.5, z: 0 } },
    } });
    towers.push({ id, x: p.x, z: p.z, wait: 0 });
    events.emit("tower.built", { x: p.x, z: p.z, gold: gold() });
    audio.play("sfx:select");
}

function sendRaider() {
    const hardy = 20 + 10 * wave();
    const id = world.spawn(`Raider${made++}`, { components: {
        Transform: { position: { x: ROAD[0][0], y: 0, z: ROAD[0][1] } },
        MeshRenderer: { mesh: `humanoid?shirt=${made % 2 ? "#922b21" : "#5b2c6f"}&trousers=#1c1c22&hair=${made % 3 ? "black" : "none"}` },
        Animator: { clip: "run" },
        PathFollower: { path: "Road", speed: 1.8 + 0.2 * wave(), orient: "flat" },
        RigidBody: { kind: "kinematic" },
        Collider: { shape: "capsule", size: { x: 0.32, y: 0.55, z: 0.32 }, offset: { x: 0, y: 0.9, z: 0 } },
        Health: { current: hardy, max: hardy, team: 2 },
    } });
    raiders.push(id);
}

onStart(() => {
    seen = events.lastSeq();
    pointer = input.pointer();
    particles.preset("Bursts", "explosion", { max: 400 });
    mount(() => (
        <box position="absolute" left={0} top={0} width="100%" height="100%">
            <box position="absolute" left={14} top={12} padding={[6, 10]} radius={6} background="#00000070">
                <Label text={`Wave ${wave()}   Gold ${gold()}   Lives ${lives()}   Down ${kills()}`} color="#ffe08a" size={16} name="hud" />
            </box>
            <box position="absolute" left={14} bottom={12} padding={[6, 10]} radius={6} background="#00000060">
                <Label text={`Click or Space: a tower for ${COST} gold (not on the road)`} color="#ffffffcc" size={13} name="hint" />
            </box>
            {fallen() ? (
                <box position="absolute" left="50%" top="40%" margin={[0, 0, 0, -130]} width={260} padding={[10, 14]} radius={8} background="#000000b0">
                    <Label text={`The keep fell at wave ${wave()}`} color="#ff8a7a" size={20} name="fallen" />
                </box>
            ) : null}
        </box>
    ));
});

onTick(({ dt }) => {
    for (const e of events.since(seen)) {
        seen = e.seq;
        if (e.type === "health.depleted" && e.subject && raiders.includes(e.subject)) {
            const at = world.get(e.subject, "Transform")!.position;
            particles.burst("Bursts", 30, { at: { x: at.x, y: 1, z: at.z } });
            audio.play(`sfx:hit?volume=0.5&seed=${made % 4}`);
            world.destroy(e.subject);
            raiders = raiders.filter((r) => r !== e.subject);
            kills.set(kills() + 1);
            gold.set(gold() + 10);
        }
    }
    if (fallen()) return;
    // Raiders at the keep: a life each.
    for (const r of [...raiders]) {
        if (world.get(r, "PathFollower")!.finished) {
            world.destroy(r);
            raiders = raiders.filter((x) => x !== r);
            lives.set(lives() - 1);
            events.emit("keep.hit", { lives: lives() });
            audio.play("sfx:hurt");
            if (lives() <= 0) {
                fallen.set(true);
                events.emit("game.over", { wave: wave(), kills: kills() });
                for (const x of raiders) world.set(x, "PathFollower", { playing: false });
                return;
            }
        }
    }
    // Waves: a pause, then the wave's raiders a second apart.
    if (queued === 0 && raiders.length === 0) {
        rest -= dt;
        if (rest <= 0) {
            wave.set(wave() + 1);
            queued = 2 + 2 * wave();
            gap = 0;
            rest = 4;
            events.emit("wave.started", { wave: wave(), raiders: queued });
        }
    }
    if (queued > 0) {
        gap -= dt;
        if (gap <= 0) {
            sendRaider();
            queued--;
            gap = 1;
        }
    }
    // The cursor: the cell under the pointer once it moves, or stepped by the keys.
    const now = input.pointer();
    if (now.x !== pointer.x || now.y !== pointer.y) {
        pointer = now;
        const ground = render.unproject(now.x, now.y, "xz", 0);
        if (ground.hit && ground.point) cell = { c: Math.round(ground.point.x / CELL), r: Math.round(ground.point.z / CELL) };
    }
    if (input.pressed("cursor_x")) cell = { c: cell.c + Math.sign(input.axis("cursor_x")), r: cell.r };
    if (input.pressed("cursor_z")) cell = { c: cell.c, r: cell.r + Math.sign(input.axis("cursor_z")) };
    cell = { c: Math.max(-COLS, Math.min(COLS, cell.c)), r: Math.max(-ROWS, Math.min(ROWS, cell.r)) };
    const p = centre(cell);
    const ok = buildable(cell) && gold() >= COST;
    world.set("Cursor", "Transform", { position: { x: p.x, y: 0.06, z: p.z } });
    world.set("Cursor", "MeshRenderer", { color: ok ? { r: 0.3, g: 1, b: 0.4, a: 0.45 } : { r: 1, g: 0.3, b: 0.25, a: 0.45 } });
    if (input.pressed("build")) build();
    // Towers: each shoots the nearest raider in range when it has reloaded.
    for (const t of towers) {
        t.wait = Math.max(0, t.wait - dt);
        if (t.wait > 0) continue;
        let best: Vec3 | null = null;
        let bestD = RANGE;
        for (const r of raiders) {
            const q = world.get(r, "Transform")!.position;
            const d = Math.hypot(q.x - t.x, q.z - t.z);
            if (d < bestD) { bestD = d; best = q; }
        }
        if (!best) continue;
        const from = { x: t.x, y: 3.2, z: t.z };
        const dir = { x: best.x - from.x, y: 1.0 - from.y, z: best.z - from.z };
        combat.shoot(from, dir, { speed: 20, damage: 10, team: 1, seconds: 0.8, radius: 0.15, color: { r: 1, g: 0.9, b: 0.4, a: 1 }, name: "Bolt" });
        t.wait = RELOAD;
    }
});

expose("gold", () => gold());
expose("lives", () => lives());
expose("wave", () => wave());
expose("kills", () => kills());
expose("towers", () => towers.length);
expose("raiders", () => raiders.length);
expose("fallen", () => fallen());
expose("cursor.x", () => centre(cell).x);
expose("cursor.z", () => centre(cell).z);
