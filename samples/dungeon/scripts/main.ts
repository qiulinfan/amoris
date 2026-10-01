// A dark dungeon (docs/design/sprites.md, Light; docs/design/tilemaps.md, Sight): the map and the
// sprites are lit, so only the torches on the walls and the lantern the player carries show them;
// the crates have normal maps that catch the light. What the player cannot see is under fog, and
// what it has seen stays dimmed: tilemap.fov from the player's cell, the fog layer painted to match.
// WASD or the arrows walk.
//   pocket run dungeon
//   pocket scenario dungeon
import { events, expose, input, onStart, onTick, setClearColor, tilemap, world } from "pocket";
import type { Entity } from "pocket";

const SPEED = 4;
const SIGHT = 7;            // cells
const SEEN = 3;             // the fog layer's dimmed tile (dungeon_tiles.png's local ids: floor 0, wall 1, fog 2, seen 3)
let player = 0;
let camera = 0;
const torches: Array<{ light: Entity; flame: Entity; base: number; phase: number }> = [];
let visible = new Set<string>();
const explored = new Set<string>();
let lastCell = "";
let t = 0;

onStart(() => {
    setClearColor(0, 0, 0, 1);
    world.spawn("Level", { components: { Transform: {}, TileMap: { map: "assets/dungeon.tmj", lit: true } } });
    for (const o of tilemap.objects("Level", "things")) {
        const at = { x: o.x, y: o.y, z: 0 };
        if (o.type === "start") {
            // The player carries a lantern: a light a little in front of the floor, moving with it.
            player = world.spawn("Player", { components: { Transform: { position: at }, Sprite: { texture: "assets/hero.png", size: { x: 0.9, y: 0.9 }, filter: "nearest", lit: true, layer: 2 }, TopDown2D: { radius: 0.3, map: "Level" } } });
            world.spawn("Lantern", { parent: player, components: { Transform: { position: { x: 0.3, y: 0, z: 1.2 } }, Light: { kind: "point", color: "#ffd9a0", intensity: 2.2, range: 5 } } });
        } else if (o.type === "torch") {
            // On the wall above the cell: the bracket (lit), the flame (glowing, unlit) and its light.
            const torch = world.spawn(o.name, { components: { Transform: { position: { x: at.x, y: at.y - 0.2, z: 0 } }, Sprite: { texture: "assets/torch.png", size: { x: 0.8, y: 0.8 }, filter: "nearest", lit: true, layer: 1 } } });
            const flame = world.spawn("Flame", { parent: torch, components: { Transform: { position: { x: 0, y: 0.35, z: 0.01 } }, Sprite: { texture: "assets/flame.png", size: { x: 0.6, y: 0.7 }, additive: true, color: "#ffc070", layer: 3 } } });
            const light = world.spawn("Light", { parent: torch, components: { Transform: { position: { x: 0, y: -0.6, z: 1.0 } }, Light: { kind: "point", color: "#ff9a40", intensity: 3, range: 6 } } });
            torches.push({ light, flame, base: 3, phase: torches.length * 1.7 });
        } else if (o.type === "crate") {
            world.spawn(o.name, { components: { Transform: { position: at }, Sprite: { texture: "assets/crate.png", normal_map: "assets/crate_n.png", size: { x: 0.9, y: 0.9 }, filter: "nearest", lit: true, layer: 1 } } });
        }
    }
    camera = world.spawn("Camera", { components: { Transform: { position: { x: 5.5, y: -5.5, z: 10 } }, Camera: { orthographic: true, ortho_size: 5.5, near: 0.1, far: 50 } } });
    updateFog();
});

// The fog: cells in view are clear, cells seen before are dimmed, the rest stay black.
function updateFog(): void {
    const p = world.get(player, "Transform")!.position;
    const view = tilemap.fov("Level", { x: p.x, y: p.y }, SIGHT);
    const now = new Set<string>();
    for (const [x, y] of view.cells) {
        const key = `${x},${y}`;
        now.add(key);
        if (!visible.has(key)) tilemap.set("Level", { tile_x: x, tile_y: y }, null, "fog");
        explored.add(key);
    }
    for (const key of visible) {
        if (now.has(key)) continue;
        const [x, y] = key.split(",").map(Number);
        tilemap.set("Level", { tile_x: x, tile_y: y }, SEEN, "fog");
    }
    visible = now;
    events.emit("fog.updated", { visible: now.size, explored: explored.size });
}

onTick(({ dt }) => {
    t += dt;
    world.set(player, "TopDown2D", { velocity: { x: input.axis("move_x") * SPEED, y: input.axis("move_y") * SPEED } });
    const body = world.get(player, "TopDown2D")!;
    const cell = `${body.tile_x},${body.tile_y}`;
    if (cell !== lastCell) {
        lastCell = cell;
        updateFog();
    }
    // The camera follows; the flames flicker, each on its own beat.
    const p = world.get(player, "Transform")!.position;
    world.set(camera, "Transform", { position: { x: p.x, y: p.y, z: 10 } });
    for (const torch of torches) {
        const f = 0.82 + 0.1 * Math.sin(t * 13 + torch.phase) + 0.08 * Math.sin(t * 29 + torch.phase * 2.3);
        world.set(torch.light, "Light", { intensity: torch.base * f });
        world.set(torch.flame, "Transform", { scale: { x: 1, y: 0.9 + 0.15 * f, z: 1 } });
    }
});

expose("player.x", () => Number(world.get(player, "Transform")!.position.x.toFixed(3)));
expose("player.y", () => Number(world.get(player, "Transform")!.position.y.toFixed(3)));
expose("visible", () => visible.size);
expose("explored", () => explored.size);
expose("torches", () => torches.length);
