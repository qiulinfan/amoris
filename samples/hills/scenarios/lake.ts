// The lake is Water (docs/design/water.md): what is lighter than the water it displaces floats on
// its waves, what is heavier sinks to the bed.
//   pocket scenario hills
import { expect, scenario, terrain, water, world } from "pocket";

// The lowest ground on a coarse look: the deepest part of the lake.
function deepest() {
    let best = { x: 0, z: 0, y: Infinity };
    for (let i = -20; i <= 20; i++) {
        for (let j = -20; j <= 20; j++) {
            const g = terrain.height(i * 2.2, j * 2.2);
            if (g.height < best.y) best = { x: i * 2.2, z: j * 2.2, y: g.height };
        }
    }
    return best;
}

scenario("a light crate floats on the lake's waves and a heavy one sinks to the bed", (g) => {
    let spot = { x: 0, z: 0, y: 0 };
    let cork = 0, stone = 0;
    g.check(() => {
        spot = deepest();
        const surface = water.height(spot.x, spot.z);
        expect(surface.path).toBe("/Lake");
        expect(surface.height! - spot.y).toBeGreaterThan(1.5);            // deep enough to sink into
        const crate = (name: string, x: number, mass: number) => world.spawn(name, {
            components: {
                Transform: { position: { x, y: surface.height! + 2, z: spot.z }, scale: { x: 0.5, y: 0.5, z: 0.5 } },
                MeshRenderer: { mesh: "cube", color: { r: 0.6, g: 0.45, b: 0.25, a: 1 } },
                RigidBody: { kind: 0, mass },
                Collider: { shape: 0, size: { x: 0.25, y: 0.25, z: 0.25 } },
            },
        });
        cork = crate("Cork", spot.x - 0.6, 0.1);      // the 0.125 cubic units of water it can displace weigh 0.25
        stone = crate("Stone", spot.x + 0.6, 0.5);
    }, "two crates over the deepest water");
    g.wait(4);
    g.check(() => {
        const c = world.get(cork, "Transform")!.position;
        const s = world.get(stone, "Transform")!.position;
        expect(Math.abs(c.y - water.height(c.x, c.z).height!)).toBeLessThan(0.3);   // on the surface
        expect(water.under(s)).toBe(true);
        expect(s.y - terrain.height(s.x, s.z).height).toBeLessThan(0.6);          // down on the bed
    }, "one floats, one sank");
});

scenario("the player put down in the deep lake swims with its head out", (g) => {
    let player = 0;
    g.check(() => {
        const spot = deepest();
        player = world.find("Player") ?? 0;
        world.set(player, "Transform", { position: { x: spot.x, y: water.height(spot.x, spot.z).height! - 1, z: spot.z } });
    }, "in the deepest water");
    g.wait(2);
    g.check(() => {
        const c = world.get(player, "Character")!;
        const p = world.get(player, "Transform")!.position;
        const top = p.y + c.height / 2;
        expect(c.swimming).toBe(true);
        expect(c.grounded).toBe(false);
        // Its head out: the top of the capsule over the surface, its centre under it.
        expect(top).toBeGreaterThan(water.height(p.x, p.z).height!);
        expect(p.y).toBeLessThan(water.height(p.x, p.z).height!);
    }, "swimming");
});
