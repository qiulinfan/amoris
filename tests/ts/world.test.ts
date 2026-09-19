import { events, world } from "pocket";
import { expect, test } from "pocket/test";

test("spawn, get, set and describe", () => {
    const level = world.spawn("Level", { components: { Transform: {} } });
    const player = world.spawn("Player", { parent: level, components: { Health: { current: 40 } } });
    expect(world.describe(player).path).toBe("/Level/Player");
    expect(world.get(player, "Health")!.max).toBe(100);
    world.set(player, "Health", { max: 250 });
    expect(world.get(player, "Health")!.current).toBe(40);
    expect(world.get(player, "Health")!.max).toBe(250);
    expect(world.get(player, "Camera")).toBeUndefined();
    expect(world.find("/Level/Player")).toBe(player);
    expect(world.find("Nope")).toBeUndefined();
});

test("tree and query", () => {
    for (let i = 0; i < 3; i++) world.spawn("Crate", { parent: "/Level", components: { Transform: { position: { x: i, y: 0, z: 0 } } } });
    const rows = world.query({ with: ["Transform"], name: "Crate*", under: "/Level" });
    expect(rows.length).toBe(3);
    expect(rows[2].Transform!.position.x).toBe(2);
    const text = world.tree({ depth: 1 });
    expect(text).toContain("- Level #");
    expect(text).toContain("Crate_3 #");
    expect(world.tree({ depth: 0 })).toContain("children)");
});

test("events carry causes", () => {
    const seq = events.emit("test.cause", { a: 1 });
    const effect = events.emit("test.effect", null, { cause: seq });
    const list = events.since(seq - 1, { type: "test." });
    expect(list.length).toBe(2);
    expect(list[1].cause).toBe(seq);
    expect(effect > seq).toBeTruthy();
});

test("errors surface as exceptions", () => {
    expect(() => world.get(999999, "Health")).toThrow();
    expect(() => world.spawn("bad/name")).toThrow();
});

test("scene round trip", () => {
    const scene = world.save();
    const n = world.summary().entities;
    expect(world.load(scene)).toBe(n);
    expect(world.save()).toEqual(scene);
});
