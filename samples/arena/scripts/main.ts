// Two players, one ball: Red (player 0) scores in the north goal, Blue (player 1) in the south.
// Run it as a lockstep game (docs/design/networking.md): one peer hosts, the other joins, and
// each moves its own player with WASD or a pad; every peer runs the whole match.
//   pocket run arena -- --net-host 7777
//   pocket run arena -- --net-join 127.0.0.1:7777
// Without a network game only Red moves.
import { events, expose, input, onStart, onTick, world } from "pocket";

const SPEED = 6;
const players = ["Red", "Blue"];
const starts = [{ x: 0, y: 0.9, z: 6 }, { x: 0, y: 0.9, z: -6 }];
let ids: number[] = [];
let ball = 0;
let seen = 0;
const score = [0, 0];

function kickOff(): void {
    world.set(ball, "Transform", { position: { x: 0, y: 0.5, z: 0 } });
    world.set(ball, "Velocity", { linear: { x: 0, y: 0, z: 0 }, angular: { x: 0, y: 0, z: 0 } });
    ids.forEach((id, p) => world.set(id, "Transform", { position: starts[p] }));
}

onStart(() => {
    ids = players.map((n) => world.find(n) ?? 0);
    ball = world.find("Ball") ?? 0;
    seen = events.lastSeq();
});

onTick(() => {
    // Every player's own actions move their own character.
    ids.forEach((id, p) => {
        const c = world.get(id, "Character")!;
        world.set(id, "Character", { velocity: { x: input.axis("move_x", p) * SPEED, y: c.velocity.y, z: input.axis("move_z", p) * SPEED } });
    });
    // A goal: the ball in a goal mouth's trigger.
    for (const e of events.since(seen)) {
        seen = e.seq;
        if (e.type !== "trigger.enter") continue;
        const d = e.data as { a: string; b: string };
        if (![d.a, d.b].includes("/Ball")) continue;
        const goal = [d.a, d.b].find((p) => p.startsWith("/Goal"));
        if (!goal) continue;
        const scorer = goal === "/GoalNorth" ? 0 : 1;
        score[scorer]++;
        events.emit("goal", { player: scorer, red: score[0], blue: score[1] });
        kickOff();
    }
});

expose("red", () => score[0]);
expose("blue", () => score[1]);
expose("ball.x", () => Number(world.get(ball, "Transform")!.position.x.toFixed(3)));
expose("ball.z", () => Number(world.get(ball, "Transform")!.position.z.toFixed(3)));
expose("red.x", () => Number(world.get(ids[0], "Transform")!.position.x.toFixed(3)));
expose("red.z", () => Number(world.get(ids[0], "Transform")!.position.z.toFixed(3)));
expose("blue.x", () => Number(world.get(ids[1], "Transform")!.position.x.toFixed(3)));
expose("blue.z", () => Number(world.get(ids[1], "Transform")!.position.z.toFixed(3)));
