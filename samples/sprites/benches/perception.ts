// Perception benchmarks for the platformer (docs/design/scenarios.md, Perception benchmarks):
// what an agent asks about a jump, the ground and the picture, answered through the instruments
// and metered against frame-by-frame vision.
//   pocket bench sprites
import { bench } from "pocket";

bench("how high does the player jump", (b) => {
    b.check(() => b.ask("recorder.start", { ticks: 600 }), "start recording");
    b.wait(0.2);
    b.press("jump");
    b.wait(1.2);
    b.measure("the recorded height over the jump", () => {
        const track = b.ask<{ ticks: number[]; values: number[] }>("recorder.track", { entity: "Player", component: "Transform", field: "position.y", every: 2 });
        const base = track.values[0];
        return Math.round((Math.max(...track.values) - base) * 100) / 100;
    });
    // Jump speed 10.5 against gravity 24 (samples/sprites): v^2 / 2g, a little less with the discrete step.
    b.verify(() => (10.5 * 10.5) / (2 * 24), (a, t) => Math.abs((a as number) - (t as number)) < 0.15);
});

bench("is the player grounded after landing", (b) => {
    b.wait(0.2);
    b.press("jump");
    b.wait(1.5);
    b.measure("one component", () => b.ask<{ grounded: boolean }>("world.get", { entity: "Player", component: "Body2D" }).grounded);
    b.verify(() => b.state("player.grounded"));
});

bench("what does the camera see", (b) => {
    b.wait(0.1);
    b.measure("render.visible", () => b.ask<{ visible: Array<{ name?: string }> }>("render.visible", { limit: 10 }).visible.map((v) => v.name ?? "").sort());
    b.verify(() => ["Level", "Player"], (a, t) => (t as string[]).every((n) => (a as string[]).includes(n)));
});

bench("where is the player after a second of walking", (b) => {
    b.hold("move_x", 1.0);
    b.measure("one component", () => Math.round(b.ask<{ position: { x: number } }>("world.get", { entity: "Player", component: "Transform" }).position.x * 10) / 10);
    b.verify(() => b.state("player.x"), (a, t) => Math.abs((a as number) - (t as number)) < 0.2);
});
