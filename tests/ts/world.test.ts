import { events, input, world } from "pocket";
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

test("prefabs instantiate with overrides and save back", () => {
    const base = world.spawn("Base");
    const fragment = {
        format: "pocket-scene",
        entities: [{ name: "Turret", components: { Transform: { position: { x: 1, y: 0, z: 0 } }, Health: { current: 10, max: 10 } }, children: [{ name: "Barrel", components: { Transform: { position: { x: 0, y: 0.5, z: 0 } } } }] }],
    } as const;
    const a = world.instantiate(fragment as never, { parent: base });
    const b = world.instantiate(fragment as never, { parent: base, name: "Turret2", components: { Transform: { position: { z: 5 } }, Health: { current: 3 } } });
    expect(world.describe(a).path).toBe("/Base/Turret");
    expect(world.describe(b).path).toBe("/Base/Turret2");
    expect(world.children(b).length).toBe(1);
    expect(world.get(b, "Transform")!.position.x).toBe(1);  // untouched by the override
    expect(world.get(b, "Transform")!.position.z).toBe(5);
    expect(world.get(b, "Health")!.current).toBe(3);
    expect(world.get(b, "Health")!.max).toBe(10);
    // WorldTransform is propagated immediately for the new subtree.
    world.set(base, "Transform", { position: { x: 100, y: 0, z: 0 } });
    const barrel = world.find("/Base/Turret2/Barrel")!;
    expect(world.describe(barrel).components.Transform!.position.y).toBe(0.5);
});

test("pack and unpack move numbers without JSON", () => {
    const ids: number[] = [];
    for (let i = 0; i < 5; i++) ids.push(world.spawn(`P${i}`, { components: { Transform: { position: { x: i, y: 0, z: 0 } }, Velocity: { linear: { x: 1, y: 2, z: 3 } } } }));
    const packed = world.pack("Transform", ["position", "scale.y"], { with: ["Velocity"], name: "P*" });
    expect(packed.count).toBe(5);
    expect(packed.stride).toBe(4);
    expect(packed.layout.position).toBe(0);
    expect(packed.layout["scale.y"]).toBe(3);
    expect(packed.data.length).toBe(20);
    let sum = 0;
    for (let i = 0; i < packed.count; i++) {
        sum += packed.data[i * packed.stride];
        packed.data[i * packed.stride + 1] = 10 + i;   // position.y
        packed.data[i * packed.stride + 3] = 2;        // scale.y
    }
    expect(sum).toBe(0 + 1 + 2 + 3 + 4);
    world.unpack();
    const idx = packed.ids.indexOf(ids[3]);
    expect(idx >= 0).toBeTruthy();
    expect(world.get(ids[3], "Transform")!.position.y).toBe(10 + idx);
    expect(world.get(ids[3], "Transform")!.scale.y).toBe(2);
    expect(world.get(ids[3], "Transform")!.position.x).toBe(3);  // untouched
    expect(() => world.pack("Transform", ["nope"])).toThrow();
    expect(() => world.pack("MeshRenderer", ["mesh"])).toThrow();  // strings are not packable
});

test("input actions map keys and synthetic holds", () => {
    input.map({ jump: ["Space", "pad:a"], move_x: { negative: ["A", "Left"], positive: ["D", "Right"], axis: ["pad:leftx"] } });
    expect(input.axis("move_x")).toBe(0);
    expect(input.down("jump")).toBe(false);
    input.hold({ action: "move_x" }, 3);   // holds D (the first positive key)
    input.press({ key: "Space" });
    const a = input.actions();
    expect(a.move_x.value).toBe(1);
    expect(a.move_x.down).toBe(true);
    expect(a.move_x.pressed).toBe(true);
    expect(a.jump.down).toBe(true);
    const d = input.describe();
    expect(d.move_x.negative!.length).toBe(2);
    expect(d.move_x.axis![0]).toBe("pad:leftx");
    expect(() => input.hold({ action: "nope" })).toThrow();
});
