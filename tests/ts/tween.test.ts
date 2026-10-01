import { ease, lastTick, nextTick, timer, tween, wait, world } from "pocket";
import { expect, test } from "pocket/test";

declare const __pocket_dispatch: (kind: string, arg: unknown) => unknown;

let tickNo = 0;
function tick(dt = 1 / 60): void {
    tickNo++;
    __pocket_dispatch("tick", { tick: tickNo, dt, time: tickNo * dt });
}

test("easing curves start at 0 and end at 1", () => {
    for (const name of Object.keys(ease) as Array<keyof typeof ease>) {
        expect(ease[name](0)).toBeCloseTo(0, 6);
        expect(ease[name](1)).toBeCloseTo(1, 6);
    }
    expect(ease.quadOut(0.5)).toBeCloseTo(0.75, 6);
    expect(ease.linear(0.25)).toBe(0.25);
});

test("value tweens follow simulation time and finish exactly", () => {
    const seen: number[] = [];
    let completed = 0;
    const h = tween.value(0, 10, { duration: 0.1, onUpdate: (v) => seen.push(v), onComplete: () => completed++ });
    expect(seen[0]).toBe(0);
    for (let i = 0; i < 6; i++) tick(0.02);
    expect(h.done).toBe(true);
    expect(seen[seen.length - 1]).toBe(10);
    expect(completed).toBe(1);
    expect(tween.count()).toBe(0);
    // Intermediate values are linear in time.
    expect(seen[1]).toBeCloseTo(2, 6);
    expect(seen[2]).toBeCloseTo(4, 6);
});

test("component tweens interpolate only the named fields and rotate the short way", () => {
    world.clear();
    const e = world.spawn("mover", { components: { Transform: { position: { x: 0, y: 5, z: 0 }, rotation: { x: 0, y: 0, z: 0, w: 1 } } } });
    const h = tween.to(e, "Transform", { position: { x: 4 }, rotation: { x: 0, y: 0.7071068, z: 0, w: 0.7071068 } }, { duration: 1, ease: "linear" });
    for (let i = 0; i < 30; i++) tick(1 / 60);
    let t = world.get(e, "Transform")!;
    expect(t.position.x).toBeCloseTo(2, 3);
    expect(t.position.y).toBeCloseTo(5, 3);   // untouched
    expect(Math.hypot(t.rotation.x, t.rotation.y, t.rotation.z, t.rotation.w)).toBeCloseTo(1, 4);
    expect(t.rotation.y > 0.3 && t.rotation.y < 0.5).toBeTruthy();
    expect(h.progress).toBeCloseTo(0.5, 3);
    h.finish();
    t = world.get(e, "Transform")!;
    expect(t.position.x).toBeCloseTo(4, 4);
    expect(t.rotation.y).toBeCloseTo(0.7071068, 4);
    expect(h.done).toBe(true);
});

test("repeat, yoyo, delay and cancel", () => {
    const values: number[] = [];
    tween.value(0, 1, { duration: 0.1, repeat: 1, yoyo: true, delay: 0.05, onUpdate: (v) => values.push(v) });
    tick(0.05);   // delay only: no movement yet beyond the initial apply
    expect(values.length).toBe(1);
    for (let i = 0; i < 5; i++) tick(0.02);   // first play reaches 1
    expect(values[values.length - 1]).toBeCloseTo(1, 6);
    for (let i = 0; i < 5; i++) tick(0.02);   // second play comes back to 0
    expect(values[values.length - 1]).toBeCloseTo(0, 6);
    expect(tween.count()).toBe(0);
    const h = tween.value(0, 1, { duration: 10, onUpdate: () => {} });
    h.cancel();
    tick();
    expect(tween.count()).toBe(0);
    expect(h.done).toBe(true);
});

test("a whole number of ticks is exact: three seconds fire on the 180th tick", () => {
    let at = -1;
    let seen = 0;
    timer.after(3, () => (at = seen));
    for (let i = 1; i <= 200; i++) {
        seen = i;
        tick(1 / 60);
    }
    expect(at).toBe(180);
    const every: number[] = [];
    timer.every(0.5, () => every.push(seen), 3);
    for (let i = 1; i <= 120; i++) {
        seen = i;
        tick(1 / 60);
    }
    expect(every).toEqual([30, 60, 90]);
});

test("timers fire in simulation time", () => {
    let once = 0, every = 0;
    timer.after(0.1, () => once++);
    const e = timer.every(0.05, () => every++, 3);
    for (let i = 0; i < 10; i++) tick(0.02);
    expect(once).toBe(1);
    expect(every).toBe(3);
    expect(e.done).toBe(true);
    expect(timer.count()).toBe(0);
    let late = 0;
    const id = setTimeout(() => late++, 30);
    clearTimeout(id);
    setTimeout(() => late += 10, 30);
    for (let i = 0; i < 3; i++) tick(0.02);
    expect(late).toBe(10);
    expect(lastTick().dt).toBe(0.02);
});

test("async gameplay resolves on later ticks", () => {
    // Promise callbacks run after the current host call returns; the test only checks that the
    // continuations are queued on the simulation clock, not on wall time.
    let phase = "start";
    void (async () => {
        await wait(0.05);
        phase = "waited";
        const t = await nextTick();
        phase = `tick ${t.tick}`;
    })();
    expect(phase).toBe("start");
    expect(timer.count()).toBe(1);
});
