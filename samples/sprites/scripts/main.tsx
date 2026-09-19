// A 2D game on sprites (docs/design/sprites.md): an orthographic camera, a ground row cut from a
// two-tile sheet, a player moved by actions, coins that bob on a tween and are collected on
// contact, a score in the HUD and in the exposed state.
//   pocket run sprites
//   pocket run sprites -- --headless --frames 120 --json
import { Label, events, expose, input, log, mount, onStart, onTick, setClearColor, signal, tween, world } from "pocket";

const score = signal(0);
const coins = new Set<number>();
let player = 0;
let facingLeft = false;

onStart(() => {
    setClearColor(0.45, 0.7, 0.95, 1);
    world.spawn("Camera", { components: { Transform: { position: { x: 0, y: 0, z: 10 } }, Camera: { orthographic: true, ortho_size: 5, near: 0.1, far: 50 } } });
    // Ground: 20 tiles from a 32x16 sheet (grass is the left half, dirt the right).
    const ground = world.spawn("Ground");
    for (let i = 0; i < 20; i++) {
        const x = i - 9.5;
        world.spawn(`tile${i}`, { parent: ground, components: { Transform: { position: { x, y: -4, z: 0 } }, Sprite: { texture: "assets/tiles.png", uv: { x: 0, y: 0, z: 0.5, w: 1 }, filter: "nearest" } } });
        world.spawn(`dirt${i}`, { parent: ground, components: { Transform: { position: { x, y: -5, z: 0 } }, Sprite: { texture: "assets/tiles.png", uv: { x: 0.5, y: 0, z: 1, w: 1 }, filter: "nearest" } } });
    }
    player = world.spawn("Player", { components: { Transform: { position: { x: 0, y: -3, z: 0 } }, Sprite: { texture: "assets/player.png", layer: 2, filter: "nearest" } } });
    for (let i = 0; i < 6; i++) {
        // Along the ground where the player walks, bobbing a little; the last two float higher.
        const y = i < 4 ? -3 : -1.5;
        const id = world.spawn(`Coin${i}`, { components: { Transform: { position: { x: -6 + i * 2.4, y, z: 0 } }, Sprite: { texture: "assets/coin.png", size: { x: 0.5, y: 0.5 }, layer: 1, filter: "nearest" } } });
        coins.add(id);
        tween.to(id, "Transform", { position: { y: y + 0.3 } }, { duration: 0.8, ease: "sineInOut", repeat: Infinity, yoyo: true, delay: i * 0.1 });
    }
    mount(() => (
        <box position="absolute" left={12} top={12} padding={[6, 10]} radius={6} background="#00000080" name="hud">
            <Label text={`Score ${score()}   Coins left ${coins.size}`} color="#ffffff" size={16} name="score" />
        </box>
    ));
    log("sprites ready", { coins: coins.size });
});

onTick((t) => {
    const speed = 6;
    const dx = input.axis("move_x") * speed * t.dt;
    const dy = input.axis("move_y") * speed * t.dt;
    const p = world.get(player, "Transform")!.position;
    const x = Math.max(-9.5, Math.min(9.5, p.x + dx));
    const y = Math.max(-3.4, Math.min(4.5, p.y + dy));
    if (dx !== 0 || dy !== 0) world.set(player, "Transform", { position: { x, y } });
    if (dx < 0 && !facingLeft) { facingLeft = true; world.set(player, "Sprite", { flip_x: true }); }
    if (dx > 0 && facingLeft) { facingLeft = false; world.set(player, "Sprite", { flip_x: false }); }
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
expose("player.x", () => Number(world.get(player, "Transform")?.position.x.toFixed(2) ?? 0));
