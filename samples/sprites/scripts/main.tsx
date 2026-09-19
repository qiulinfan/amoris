// A 2D platformer on sprites (docs/design/sprites.md): an orthographic camera, a level from a
// Tiled map (ground, a ledge and a one-way plank drawn by the TileMap component, the player and
// the coins placed as map objects), a player with a Body2D that gravity pulls onto the tiles and
// actions push and jump, a walk cycle from a sheet clip, spinning coins that bob on a tween and
// are collected on contact, a score in the HUD and in the exposed state.
//   pocket run sprites
//   pocket run sprites -- --headless --frames 120 --json
import { Label, events, expose, input, log, mount, onStart, onTick, setClearColor, signal, sprites, tilemap, tween, world } from "pocket";

const score = signal(0);
const coins = new Set<number>();
let player = 0;
let level = 0;
let lift = 0;
let facingLeft = false;
let walking = false;

onStart(() => {
    setClearColor(0.45, 0.7, 0.95, 1);
    world.spawn("Camera", { components: { Transform: { position: { x: 0, y: 0, z: 10 } }, Camera: { orthographic: true, ortho_size: 5, near: 0.1, far: 50 } } });
    // The level is a Tiled map: 20x10 tiles, the entity at its top-left corner (docs/design/tilemaps.md).
    level = world.spawn("Level", { components: { Transform: { position: { x: -10, y: 4.5, z: 0 } }, TileMap: { map: "assets/level.tmj" } } });
    const spawns = tilemap.objects(level, "spawns");
    const start = spawns.find((o) => o.name === "player") ?? { x: 0, y: -3 };
    player = world.spawn("Player", { components: { Transform: { position: { x: start.x, y: start.y, z: 0 } }, Sprite: { texture: "assets/player.png", layer: 2, filter: "nearest" }, Body2D: { size: { x: 0.4, y: 0.5 } } } });
    sprites.play(player, "idle");
    // A lift: a kinematic Body2D riding up and down a rail on the left; the player jumps onto it
    // and is carried (docs/design/tilemaps.md, 2D physics). One-way, so it passes the walking
    // player from below instead of shoving it.
    lift = world.spawn("Lift", { components: { Transform: { position: { x: -8.5, y: -3.2, z: 0 } }, Sprite: { texture: "assets/tiles.png", size: { x: 1.5, y: 0.3 }, uv: { x: 0.4, y: 0, z: 0.6, w: 0.375 }, layer: 1, filter: "nearest" }, Body2D: { kinematic: true, one_way: true, size: { x: 0.75, y: 0.15 }, velocity: { x: 0, y: 1 } } } });
    // A ball that bounces (restitution 0.7) and a puck that slides to a stop (friction 5), both
    // ghosts to the other bodies so they never get in the player's way (docs/design/tilemaps.md,
    // Friction and restitution).
    world.spawn("Ball", { components: { Transform: { position: { x: 0.3, y: 0.5, z: 0 } }, Sprite: { texture: "assets/tiles.png", size: { x: 0.4, y: 0.4 }, uv: { x: 0.6, y: 0, z: 0.8, w: 1 }, layer: 1, filter: "nearest" }, Body2D: { size: { x: 0.2, y: 0.2 }, restitution: 0.7, collide_bodies: false } } });
    world.spawn("Puck", { components: { Transform: { position: { x: 2.4, y: -3.3, z: 0 } }, Sprite: { texture: "assets/tiles.png", size: { x: 0.4, y: 0.4 }, uv: { x: 0.2, y: 0, z: 0.4, w: 1 }, layer: 1, filter: "nearest" }, Body2D: { size: { x: 0.2, y: 0.2 }, friction: 5, collide_bodies: false, grounded: true, velocity: { x: -2.5, y: 0 } } } });
    let i = 0;
    for (const o of spawns.filter((o) => o.type === "coin")) {
        const id = world.spawn(`Coin${i}`, { components: { Transform: { position: { x: o.x, y: o.y, z: 0 } }, Sprite: { texture: "assets/coin.png", size: { x: 0.5, y: 0.5 }, layer: 1, filter: "nearest" } } });
        sprites.play(id, "coin", { speed: 1 + i * 0.15 });
        coins.add(id);
        const bob = Number(o.properties.bob ?? 0.3);
        tween.to(id, "Transform", { position: { y: o.y + bob } }, { duration: 0.8, ease: "sineInOut", repeat: Infinity, yoyo: true, delay: i * 0.1 });
        i++;
    }
    mount(() => (
        <box position="absolute" left={12} top={12} padding={[6, 10]} radius={6} background="#00000080" name="hud">
            <Label text={`Score ${score()}   Coins left ${coins.size}`} color="#ffffff" size={16} name="score" />
        </box>
    ));
    log("sprites ready", { coins: coins.size });
});

onTick((t) => {
    // The lift turns around at the ends of its rail.
    const liftY = world.get(lift, "Transform")!.position.y;
    const liftV = world.get(lift, "Body2D")!.velocity.y;
    if (liftY > -1.0 && liftV > 0) world.set(lift, "Body2D", { velocity: { x: 0, y: -1 } });
    else if (liftY < -3.2 && liftV < 0) world.set(lift, "Body2D", { velocity: { x: 0, y: 1 } });
    const speed = 6;
    const body = world.get(player, "Body2D")!;
    const p = world.get(player, "Transform")!.position;
    let vx = input.axis("move_x") * speed;
    if ((p.x <= -9.5 && vx < 0) || (p.x >= 9.5 && vx > 0)) vx = 0;   // the level's edges
    let vy = body.velocity.y;
    if (input.pressed("jump") && body.grounded) {
        vy = 10.5;                                                    // rises 2.3 units: onto the ledge and the plank (1.5 up) with room
        events.emit("player.jumped", { x: Number(p.x.toFixed(2)) }, { subject: player });
    }
    // The engine moves the body: gravity, walls, floors and one-way planks (docs/design/tilemaps.md).
    world.set(player, "Body2D", { velocity: { x: vx, y: vy } });
    const moving = vx !== 0 && body.grounded;
    if (moving !== walking) { walking = moving; sprites.play(player, walking ? "walk" : "idle"); }
    if (vx < 0 && !facingLeft) { facingLeft = true; world.set(player, "Sprite", { flip_x: true }); }
    if (vx > 0 && facingLeft) { facingLeft = false; world.set(player, "Sprite", { flip_x: false }); }
    const x = p.x, y = p.y;
    void t;
    for (const id of coins) {
        const c = world.get(id, "Transform");
        if (!c) { coins.delete(id); continue; }
        if (Math.hypot(c.position.x - x, c.position.y - y) < 0.7) {
            coins.delete(id);
            tween.cancelAll(id);
            world.destroy(id);
            score.update((s) => s + 1);
            events.emit("coin.collected", { score: score() }, { subject: player });
            tween.scale(player, 1.3, { duration: 0.1, yoyo: true, repeat: 1, ease: "quadOut" });
        }
    }
});

expose("score", () => score());
expose("coins", () => coins.size);
let ballBounces = 0;
let bounceSeq = events.lastSeq();   // count from this run's start: the log runs on across an env.reset
onTick(() => {
    for (const e of events.since(bounceSeq, { type: "body2d.bounced", limit: 50 })) {
        bounceSeq = e.seq;
        if ((e.data as { path?: string }).path === "/Ball") ballBounces++;
    }
});
expose("ball.bounces", () => ballBounces);
expose("ball.y", () => Number((world.get(world.find("Ball") ?? 0, "Transform")?.position.y ?? 0).toFixed(3)));
expose("ball.grounded", () => world.get(world.find("Ball") ?? 0, "Body2D")?.grounded ?? false);
expose("puck.x", () => Number((world.get(world.find("Puck") ?? 0, "Transform")?.position.x ?? 0).toFixed(3)));
expose("puck.vx", () => Number((world.get(world.find("Puck") ?? 0, "Body2D")?.velocity.x ?? 0).toFixed(3)));
expose("done", () => coins.size === 0);   // the environment interface ends an episode here (docs/design/environment.md)
expose("player.x", () => Number(world.get(player, "Transform")?.position.x.toFixed(2) ?? 0));
expose("player.clip", () => world.get(player, "SpriteAnimation")?.clip ?? "");
expose("player.y", () => Number(world.get(player, "Transform")?.position.y.toFixed(2) ?? 0));
expose("player.grounded", () => world.get(player, "Body2D")?.grounded ?? false);
expose("player.riding", () => { const r = world.get(player, "Body2D")?.riding ?? 0; return r ? world.describe(r).path : ""; });
expose("player.slope", () => world.get(player, "Body2D")?.on_slope ?? 0);
expose("lift.y", () => Number(world.get(lift, "Transform")?.position.y.toFixed(2) ?? 0));
expose("level.solid_below", () => tilemap.solid(level, { x: world.get(player, "Transform")?.position.x ?? 0, y: (world.get(player, "Transform")?.position.y ?? 0) - 0.6 }));
