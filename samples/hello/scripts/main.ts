// The smallest Pocket project: a bouncing ball over a ground plane and a drifting background.
//
// Nothing here reads the wall clock or a pixel. The ball's height and bounce count are exposed,
// so `pocket run hello --headless --frames 120 --json` reports them as numbers and a stable hash,
// and the same numbers drive the sphere you see in the window.
import { expose, hsvToRgb, log, onStart, onTick, random, runtime, setClearColor, world } from "pocket";
import { Ball } from "./ball";

const ball = new Ball(3.0);
let hue = 0;
let sphere = 0;

onStart(() => {
    const info = runtime();
    log("hello from TypeScript on Pocket", { tickRate: info.tickRate, headless: info.headless, firstRandom: random() });
    world.spawn("Ground", { components: { Transform: { position: { x: 0, y: -0.05, z: 0 }, scale: { x: 8, y: 0.1, z: 8 } }, MeshRenderer: { mesh: "cube", color: { r: 0.4, g: 0.45, b: 0.4, a: 1 } } } });
    sphere = world.spawn("Ball", { components: { Transform: { position: { x: 0, y: ball.y, z: 0 } }, MeshRenderer: { mesh: "sphere", color: { r: 0.9, g: 0.5, b: 0.2, a: 1 } } } });
    world.spawn("Sun", { components: { Transform: { rotation: { x: -0.46, y: 0.2, z: 0.1, w: 0.86 } }, Light: { kind: 0, intensity: 1.2 } } });
    world.spawn("Camera", { components: { Transform: { position: { x: 0, y: 2.5, z: 7 }, rotation: { x: -0.13, y: 0, z: 0, w: 0.99 } }, Camera: { fov_degrees: 50 } } });
});

onTick(({ tick, dt }) => {
    hue = (hue + dt * 0.05) % 1;
    if (ball.step(dt)) {
        log("bounce", { tick, bounces: ball.bounces, speed: ball.velocity });
    }
    world.set(sphere, "Transform", { position: { y: ball.y + 0.5 } });
    const [r, g, b] = hsvToRgb(hue, 0.45, 0.55);
    setClearColor(r, g, b, 1);
});

expose("hue", () => Number(hue.toFixed(4)));
expose("ball.y", () => Number(ball.y.toFixed(4)));
expose("ball.grounded", () => ball.grounded);
expose("bounces", () => ball.bounces);
