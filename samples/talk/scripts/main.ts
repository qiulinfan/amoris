// A conversation (docs/design/dialogue.md): walk up to the guard and press E to talk. The talk is
// a script in dialogue/guard.dialogue.json: lines, a choice whose toll needs the gold, a question
// that loops back, and an event the game answers by opening the gate. Space moves past a line,
// the number keys (or a click) take a choice.
//   pocket run talk
//   pocket scenario talk
import { dialogue, events, expose, input, onStart, onTick, setClearColor, world } from "pocket";
import type { Conversation } from "pocket";

const SPEED = 4;
let player = 0;
let guard = 0;
let gate = 0;
let talking: Conversation | null = null;
let gold = 7;
let paid = false;
let seen = 0;

const box = (x: number, y: number, w: number, h: number, color: string) => ({ Transform: { position: { x, y, z: 0 } }, Sprite: { size: { x: w, y: h }, color } });

onStart(() => {
    setClearColor(0.42, 0.62, 0.36, 1);
    world.spawn("Camera", { components: { Transform: { position: { x: 0, y: 0, z: 10 } }, Camera: { orthographic: true, ortho_size: 5 } } });
    world.spawn("Wall", { components: box(4, 0, 0.6, 10, "#6d6a66") });
    gate = world.spawn("Gate", { components: box(4, 0, 0.7, 1.6, "#8a5a2b") });
    guard = world.spawn("Guard", { components: box(3.1, 1.2, 0.7, 0.9, "#c0392b") });
    player = world.spawn("Player", { components: box(-4, 0, 0.6, 0.8, "#2e86de") });
    seen = events.lastSeq();
});

onTick(({ dt }) => {
    for (const e of events.since(seen)) {
        seen = e.seq;
        if (e.type === "gate.open" && gate) {
            world.destroy(gate);
            gate = 0;
        }
    }
    if (talking) return;   // the box has the keys while the guard talks
    const p = world.get(player, "Transform")!.position;
    let x = p.x + input.axis("move_x") * SPEED * dt;
    const y = p.y + input.axis("move_y") * SPEED * dt;
    if (gate && x > 3.3) x = Math.min(x, Math.max(p.x, 3.3));   // the closed gate holds
    world.set(player, "Transform", { position: { x, y } });
    const g = world.get(guard, "Transform")!.position;
    if (input.pressed("talk") && Math.hypot(g.x - x, g.y - y) < 1.8) {
        talking = dialogue.start("dialogue/guard.dialogue.json", { gold, paid });
        dialogue.show(talking, { onEnd: (c) => {
            gold = Number(c.vars.gold);
            paid = c.vars.paid === true;
            talking = null;
        } });
    }
});

expose("talking", () => talking !== null);
expose("gold", () => gold);
expose("gate_open", () => gate === 0);
expose("player.x", () => Number(world.get(player, "Transform")!.position.x.toFixed(2)));
