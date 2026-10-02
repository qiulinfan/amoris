// Guards (docs/design/behavior.md): three guards walk their rounds in a walled courtyard, each a
// built-in humanoid whose Animator walks and runs it by its speed (docs/design/animation.md,
// Locomotion), and each a
// Behavior on the entity: it patrols a Path, runs after the player when it sees it (within 7 units,
// across 150 degrees of where it faces, no wall between), goes to where it last saw it once it has
// lost sight of it, and after 4 seconds without seeing it goes back to its round. WASD or the left stick walks the player to the golden vault at
// the north wall; a guard that reaches the player sends it back to the start.
import { events, expose, input, nav, onStart, onTick, repro, world } from "pocket";

const START = { x: 0, y: 0.9, z: 12 };
const ROUTES = ["RouteWest", "RouteMiddle", "RouteEast"];
let player = 0;
let body = 0;
let guards: number[] = [];
let seen = 0;
let caught = 0;
let complete = false;

onStart(() => {
    player = world.find("Player") ?? 0;
    body = world.find("Player/Body") ?? 0;
    nav.bake({ min: { x: -14.5, y: -1, z: -14.5 }, max: { x: 14.5, y: 3, z: 14.5 }, cell: 0.5, agent_radius: 0.35, agent_height: 1.8 });
    guards = ROUTES.map((route, i) => {
        const at = world.get(world.find(`Routes/${route}`)!, "Path")!.points[0];
        return world.spawn(`Guard${i + 1}`, { components: {
            Transform: { position: { x: at.x, y: 0, z: at.z }, scale: { x: 0.9, y: 0.9, z: 0.9 } },
            MeshRenderer: { mesh: "humanoid?shirt=#b03a2e&trousers=#1e1e24&hair=black" },
            Animator: { locomotion: true, walk_speed: 1.3, run_speed: 3.8 },
            NavAgent: { speed: 2, face: true },
            Behavior: {
                target: "Player",
                sight: 7,
                fov: 150,
                eye: 1.5,
                states: [
                    { name: "patrol", move: "patrol", path: `Routes/${route}`, speed: 2 },
                    { name: "chase", move: "follow", speed: 4.2, event: "guard.spotted" },
                    { name: "search", move: "seek", speed: 4 },
                ],
                transitions: [
                    { from: "patrol", to: "chase", when: "sees" },
                    { from: "chase", to: "search", when: "not sees" },
                    { from: "search", to: "chase", when: "sees" },
                    { from: "search", to: "patrol", when: "unseen > 4" },
                ],
            },
        } });
    });
    seen = events.lastSeq();
});

onTick(() => {
    if (!player) return;
    const vx = input.axis("move_x") * 5, vz = input.axis("move_z") * 5;
    world.set(player, "Character", { velocity: { x: vx, y: world.get(player, "Character")!.velocity.y, z: vz } });
    // The body turned to face where it walks (the humanoid faces -Z).
    if (body && Math.hypot(vx, vz) > 0.1) {
        const yaw = repro.atan2(-vx, -vz);
        world.set(body, "Transform", { rotation: { x: 0, y: repro.sin(yaw / 2), z: 0, w: repro.cos(yaw / 2) } });
    }
    const p = world.get(player, "Transform")!.position;
    // Caught: a guard within reach sends the player back to the start.
    for (const g of guards) {
        const q = world.get(g, "Transform")!.position;
        if (Math.hypot(q.x - p.x, q.z - p.z) < 0.9) {
            caught++;
            events.emit("player.caught", { by: guards.indexOf(g) + 1, caught }, { subject: player });
            world.set(player, "Transform", { position: START });
            break;
        }
    }
    // The vault is a trigger: entering it wins.
    for (const e of events.since(seen)) {
        seen = e.seq;
        if (!complete && e.type === "trigger.enter" && e.subject === player && String((e.data as { b?: string }).b).endsWith("Vault")) {
            complete = true;
            events.emit("level.complete", { caught }, { subject: player });
        }
    }
});

expose("player.x", () => Number(world.get(player, "Transform")!.position.x.toFixed(3)));
expose("player.z", () => Number(world.get(player, "Transform")!.position.z.toFixed(3)));
expose("chasing", () => guards.filter((g) => world.get(g, "Behavior")!.state !== "patrol").length);
expose("caught", () => caught);
expose("complete", () => complete);
