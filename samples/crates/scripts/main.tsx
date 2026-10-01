// 2D rigid bodies (docs/design/physics2d.md): a pyramid of crates, a wrecking ball on a rope that
// Space lets go, a seesaw on a hinge with a crate on one end, and a car on two sprung, motored
// wheels that the arrow keys drive. The HUD counts the crates knocked down in rich text.
//   pocket run crates
//   pocket scenario crates
import { Label, expose, input, mount, onStart, onTick, setClearColor, signal, world } from "pocket";
import type { Entity } from "pocket";

const down = signal(0);
const crates: Array<{ id: Entity; y: number }> = [];
let ball = 0;
let rope = 0;
let released = false;
let car = 0;
const wheels: Entity[] = [];

const at = (x: number, y: number) => ({ position: { x, y, z: 0 } });

function crate(name: string, x: number, y: number): Entity {
    return world.spawn(name, { components: { Transform: at(x, y), Sprite: { texture: "assets/crate.png", size: { x: 0.8, y: 0.8 }, filter: "nearest" }, RigidBody2D: {}, Collider2D: { size: { x: 0.4, y: 0.4 }, friction: 0.7 } } });
}

onStart(() => {
    setClearColor(0.55, 0.75, 0.95, 1);
    world.spawn("Camera", { components: { Transform: { position: { x: 0, y: 4, z: 10 } }, Camera: { orthographic: true, ortho_size: 6, near: 0.1, far: 50 } } });
    // The ground: a static shape of its own (a Collider2D without a body), drawn as a strip.
    world.spawn("Ground", { components: { Transform: at(0, -0.5), Sprite: { size: { x: 30, y: 1 }, color: { r: 0.35, g: 0.6, b: 0.3, a: 1 } }, Collider2D: { size: { x: 15, y: 0.5 } } } });
    // A pyramid of fifteen crates, five at the bottom.
    for (let row = 0; row < 5; row++) {
        for (let i = 0; i < 5 - row; i++) {
            const x = 5.6 + row * 0.42 + i * 0.84;
            const y = 0.4 + row * 0.8;
            crates.push({ id: crate(`Crate_${row}_${i}`, x, y), y });
        }
    }
    // A wrecking ball, held up by a rope to a point over the pyramid's left, until Space lets it go.
    ball = world.spawn("Ball", { components: { Transform: at(-1, 7.5), Sprite: { texture: "assets/ball.png", size: { x: 1.2, y: 1.2 } }, RigidBody2D: { kind: "static" }, Collider2D: { shape: "circle", radius: 0.6, density: 6 }, Joint2D: { kind: "distance", other_anchor: { x: 4, y: 7.5 } } } });
    world.spawn("Pivot", { components: { Transform: at(4, 7.5), Sprite: { size: { x: 0.2, y: 0.2 }, color: { r: 0.2, g: 0.2, b: 0.2, a: 1 } } } });
    rope = world.spawn("Rope", { components: { Transform: at(1.5, 7.5), Sprite: { size: { x: 5, y: 0.06 }, color: { r: 0.25, g: 0.2, b: 0.15, a: 1 }, layer: -1 } } });
    // A seesaw: a plank on a hinge at the top of a ramp-sided block, a crate waiting on its high end.
    world.spawn("Fulcrum", { components: { Transform: at(-7, 0), Collider2D: { shape: "polygon", points: [{ x: -0.5, y: 0 }, { x: 0.5, y: 0 }, { x: 0, y: 0.8 }] }, Sprite: { size: { x: 0.6, y: 0.8 }, anchor: { x: 0.5, y: 0 }, color: { r: 0.4, g: 0.4, b: 0.45, a: 1 } } } });
    world.spawn("Seesaw", { components: { Transform: at(-7, 0.9), Sprite: { texture: "assets/plank.png", size: { x: 4, y: 0.25 }, filter: "nearest" }, RigidBody2D: {}, Collider2D: { size: { x: 2, y: 0.1 } }, Joint2D: { kind: "revolute", other_anchor: { x: -7, y: 0.9 }, enable_limit: true, lower: -0.35, upper: 0.35 } } });
    crate("Rider", -8.6, 1.6);
    // A car: a chassis on two wheels, each held to it by a sprung wheel joint whose motor the arrows drive.
    car = world.spawn("Car", { components: { Transform: at(-2.5, 1.2), Sprite: { size: { x: 2, y: 0.5 }, color: { r: 0.85, g: 0.3, b: 0.25, a: 1 } }, RigidBody2D: {}, Collider2D: { size: { x: 1, y: 0.25 }, density: 2 } } });
    for (const [name, dx] of [["WheelBack", -0.7], ["WheelFront", 0.7]] as const) {
        wheels.push(world.spawn(name, { components: { Transform: at(-2.5 + dx, 0.7), Sprite: { texture: "assets/ball.png", size: { x: 0.7, y: 0.7 }, color: { r: 0.3, g: 0.3, b: 0.3, a: 1 } }, RigidBody2D: {}, Collider2D: { shape: "circle", radius: 0.35, friction: 0.9 }, Joint2D: { kind: "wheel", body: car, other_anchor: { x: dx, y: -0.5 }, axis: { x: 0, y: 1 }, enable_spring: true, hertz: 5, damping_ratio: 0.7, enable_motor: true, max_motor_force: 30 } } }));
    }
    mount(() => (
        <box position="absolute" left={12} top={12} padding={[6, 10]} radius={6} background="#00000080" name="hud">
            <Label text={`[b]Crates down[/b]: [color=#ffcc44]${down()}[/color] of ${crates.length}`} markup outline="1 #000000" color="#ffffff" size={16} name="score" />
            <Label text={released ? "Arrows drive the car" : "Space lets the ball go; arrows drive the car"} color="#ffffffcc" size={12} name="hint" />
        </box>
    ));
});

onTick(() => {
    if (!released && input.pressed("fire")) {
        released = true;
        world.set(ball, "RigidBody2D", { kind: "dynamic" });
    }
    // The rope: a thin sprite from the pivot to the ball, turned along it.
    const b = world.get(ball, "Transform")!.position;
    const dx = b.x - 4, dy = b.y - 7.5;
    const angle = Math.atan2(dy, dx);
    world.set(rope, "Transform", { position: { x: 4 + dx / 2, y: 7.5 + dy / 2 }, rotation: { x: 0, y: 0, z: Math.sin(angle / 2), w: Math.cos(angle / 2) } });
    world.set(rope, "Sprite", { size: { x: Math.hypot(dx, dy), y: 0.06 } });
    // The wheels turn the way the arrows point (a negative speed turns clockwise: forward).
    const speed = -input.axis("move_x") * 18;
    for (const w of wheels) world.set(w, "Joint2D", { motor_speed: speed });
    // A crate is down once it has dropped half its height or tipped past 30 degrees.
    let n = 0;
    for (const c of crates) {
        const t = world.get(c.id, "Transform");
        if (!t) continue;
        const tilt = 2 * Math.asin(Math.min(1, Math.abs(t.rotation.z)));
        if (t.position.y < c.y - 0.4 || tilt > Math.PI / 6) n++;
    }
    if (n !== down()) down.set(n);
});

expose("crates_down", () => down());
expose("released", () => released);
expose("car_x", () => Number((world.get(car, "Transform")?.position.x ?? 0).toFixed(3)));
expose("car_tilt", () => {
    const q = world.get(car, "Transform")?.rotation;
    return q ? Number((2 * Math.asin(Math.max(-1, Math.min(1, q.z)))).toFixed(3)) : 0;
});
