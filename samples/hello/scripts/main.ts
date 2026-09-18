// The smallest Pocket project: a bouncing value and a drifting background color.
//
// Nothing here reads the wall clock or a pixel. The ball's height and bounce count are exposed,
// so `pocket run hello --headless --frames 120 --json` reports them as numbers and a stable hash.
import { expose, hsvToRgb, log, onStart, onTick, random, runtime, setClearColor } from "pocket";
import { Ball } from "./ball";

const ball = new Ball(3.0);
let hue = 0;

onStart(() => {
    const info = runtime();
    log("hello from TypeScript on Pocket", { tickRate: info.tickRate, headless: info.headless, firstRandom: random() });
});

onTick(({ tick, dt }) => {
    hue = (hue + dt * 0.05) % 1;
    if (ball.step(dt)) {
        log("bounce", { tick, bounces: ball.bounces, speed: ball.velocity });
    }
    const [r, g, b] = hsvToRgb(hue, 0.45, 0.55);
    setClearColor(r, g, b, 1);
});

expose("hue", () => Number(hue.toFixed(4)));
expose("ball.y", () => Number(ball.y.toFixed(4)));
expose("ball.grounded", () => ball.grounded);
expose("bounces", () => ball.bounces);
