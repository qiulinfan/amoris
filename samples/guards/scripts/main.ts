// Guards (docs/design/behavior.md): three guards walk their rounds in a walled courtyard, each a
// Behavior on the entity: it patrols a Path, runs after the player when it sees it (within 7 units,
// across 150 degrees of where it faces, no wall between), goes to where it last saw it once it has
// lost sight of it, and after 4 seconds without seeing it goes back to its round. WASD or the left stick walks the player to the golden vault at
// the north wall; a guard that reaches the player sends it back to the start.
import { events, expose, input, nav, onStart, onTick, world } from "pocket";

const START = { x: 0, y: 0.9, z: 12 };
const ROUTES = ["RouteWest", "RouteMiddle", "RouteEast"];
let player = 0;
let guards: number[] = [];
let seen = 0;
let caught = 0;
let complete = false;

onStart(() => {
    player = world.find("Player") ?? 0;
    nav.bake({ min: { x: -14.5, y: -1, z: -14.5 }, max: { x: 14.5, y: 3, z: 14.5 }, cell: 0.5, agent_radius: 0.35, agent_height: 1.8 });
    guards = ROUTES.map((route, i) => {
        const at = world.get(world.find(`Routes/${route}`)!, "Path")!.points[0];
        return world.spawn(`Guard${i + 1}`, { components: {
            Transform: { position: { x: at.x, y: 0.9, z: at.z }, scale: { x: 0.6, y: 0.9, z: 0.6 } },
            MeshRenderer: { mesh: "capsule", color: { r: 0.85, g: 0.3, b: 0.25, a: 1 } },
            NavAgent: { speed: 2, face: true },
            Behavior: {
                target: "Player",
                sight: 7,
                fov: 150,
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
    world.set(player, "Character", { velocity: { x: input.axis("move_x") * 5, y: world.get(player, "Character")!.velocity.y, z: input.axis("move_z") * 5 } });
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
