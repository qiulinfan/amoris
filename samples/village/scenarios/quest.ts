// Scenarios for the village sample (docs/design/scenarios.md): the elder's quest through the keys a
// player would press, the apples picked up by walking onto them, and the villagers at their days.
//   pocket scenario village
import { command, expect, scenario, world } from "pocket";
import type { ScenarioTools } from "pocket";

const key = (k: string) => command("input.press", { key: k });
const put = (x: number, z: number) => world.set("Player", "Transform", { position: { x, y: 0.9, z } });

// One line of the box: Space shows the rest of it, Space again moves on.
function line(g: ScenarioTools, n = 1) {
    for (let i = 0; i < n; i++) {
        g.check(() => key("Space"));
        g.wait(0.1);
        g.check(() => key("Space"));
        g.wait(0.1);
    }
}

scenario("the elder asks for his three apples", (g) => {
    g.check(() => put(1.6, 2.9), "the player beside the elder");
    g.wait(0.1);
    g.check(() => expect(g.state("near_elder")).toBe(true), "near enough to talk");
    g.press("talk");
    g.wait(0.1);
    g.check(() => expect(g.state("talking")).toBe(true), "the elder talks");
    line(g, 2);                    // welcome; the cart
    g.check(() => key("1"));       // I'll find them
    g.wait(0.1);
    line(g, 1);                    // bless you
    g.check(() => {
        expect(g.state("talking")).toBe(false);
        expect(g.state("asked")).toBe(true);
        expect(g.count("quest.started")).toBe(1);
    }, "the quest is on");
});

scenario("walking onto an apple picks it up", (g) => {
    g.check(() => put(-11, 2.5), "the player a step and a half east of an apple");
    g.hold("move_x", 0.8, -1);
    g.check(() => {
        expect(g.state("apples")).toBe(1);
        expect(g.count("apple.found")).toBe(1);
        expect(world.find("Apple0")).toBe(undefined);
    }, "picked up, and gone from the ground");
});

scenario("three apples and a word with the elder finish the quest", (g) => {
    for (const [x, z] of [[-12.5, 2.5], [11, -3.5], [-3, 15.5]]) {
        g.check(() => put(x + 0.2, z), `onto the apple at ${x}, ${z}`);
        g.wait(0.15);
    }
    g.check(() => expect(g.state("apples")).toBe(3), "all three found");
    g.check(() => put(1.6, 2.9), "back to the elder");
    g.wait(0.1);
    g.press("talk");
    g.wait(0.1);
    line(g, 2);                    // all three; the elder beams
    g.check(() => {
        expect(g.state("done")).toBe(true);
        expect(g.count("quest.complete")).toBe(1);
        expect(world.get("Elder", "Animator")!.clip).toBe("cheer");
    }, "the quest is done, and the elder cheers");
});

scenario("a sitter stands to wave as the player comes by, and sits again", (g) => {
    g.check(() => {
        const b = world.get("Sitter0", "Transform")!.position;
        put(b.x + 2, b.z + 1);
    }, "the player beside a bench");
    g.until(() => world.get("Sitter0", "Behavior")!.state === "greet", { timeout: 0.5, label: "it greets" });
    g.check(() => expect(world.get("Sitter0", "Animator")!.clip).toBe("wave"), "waving");
    g.check(() => put(0, 9), "the player walks off");
    g.until(() => world.get("Sitter0", "Behavior")!.state === "sit", { timeout: 0.5, label: "it sits again" });
});

scenario("the strollers keep to the square", (g) => {
    g.wait(20);
    g.check(() => {
        for (const n of ["Stroller0", "Stroller1", "Stroller2"]) {
            const p = world.get(n, "Transform")!.position;
            expect(Math.abs(p.x)).toBeLessThan(11);
            expect(Math.abs(p.z)).toBeLessThan(8);
        }
    }, "all three still on the cobbles");
});
