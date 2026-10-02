// Scenarios for the talk sample (docs/design/scenarios.md): the conversation through the keys a
// player would press.
//   pocket scenario talk
import { command, dialogue, expect, scenario } from "pocket";
import type { ScenarioTools } from "pocket";

const key = (k: string) => command("input.press", { key: k });

// One line: Space shows the rest of it, Space again moves on.
function line(g: ScenarioTools, n = 1) {
    for (let i = 0; i < n; i++) {
        g.check(() => key("Space"));
        g.wait(0.1);
        g.check(() => key("Space"));
        g.wait(0.1);
    }
}

scenario("asking about the castle loops back; paying the toll opens the gate", (g) => {
    g.hold("move_x", 1.6);                       // up to the guard
    g.press("talk");
    g.wait(0.1);
    g.check(() => expect(g.state("talking")).toBe(true), "the guard talks");
    line(g, 2);                                  // halt; the toll
    g.check(() => key("2"));                     // ask about the castle
    g.wait(0.1);
    line(g, 4);                                  // its two lines, then the gate's two again
    g.check(() => key("1"));                     // pay
    g.wait(0.1);
    line(g, 2);                                  // thanks; the gate creaks open
    g.check(() => {
        expect(g.state("gold")).toBe(2);
        expect(g.state("gate_open")).toBe(true);
        expect(g.state("talking")).toBe(false);
    }, "paid, the gate open, the talk over");
    g.hold("move_x", 1.0);
    g.check(() => expect(g.state<number>("player.x")).toBeGreaterThan(4.5), "through the gate");
});

scenario("walking away keeps the gate shut and the gold", (g) => {
    g.hold("move_x", 1.6);
    g.press("talk");
    g.wait(0.1);
    line(g, 2);
    g.check(() => key("3"));
    g.wait(0.1);
    g.check(() => {
        expect(g.state("talking")).toBe(false);
        expect(g.state("gold")).toBe(7);
        expect(g.state("gate_open")).toBe(false);
    }, "the talk over, nothing paid");
    g.hold("move_x", 1.0);
    g.check(() => expect(g.state<number>("player.x")).toBeLessThan(3.5), "the gate holds");
});

scenario("the guard's script is sound", (g) => {
    g.check(() => expect(dialogue.check("dialogue/guard.dialogue.json")).toEqual([]), "no missing nodes, no broken expressions");
});

scenario("every way through the guard's script, run dry", (g) => {
    g.check(() => {
        const { routes, complete } = dialogue.routes("dialogue/guard.dialogue.json");
        expect(complete).toBe(true);
        expect(routes.map((r) => r.choices[r.choices.length - 1])).toEqual(["Pay the toll (7 gold left)", "Ask about the castle", "Walk away"]);
        const [pay, ask, away] = routes;
        expect(pay.done).toBe(true);
        expect(pay.vars.gold).toBe(2);
        expect(pay.events).toEqual([{ name: "gate.open", data: {} }]);
        expect(pay.said[pay.said.length - 1]).toBe("The gate creaks open.");
        expect(ask.done).toBe(false);
        expect(ask.stopped).toBe("loops back to the choice in 'gate'");
        expect(away.done).toBe(true);
        expect(away.vars.gold).toBe(7);
        // One way through by its choices, and a toll the traveller cannot pay.
        expect(dialogue.play("dialogue/guard.dialogue.json", [1, 0]).events.length).toBe(1);
        expect(dialogue.routes("dialogue/guard.dialogue.json", { gold: 3 }).routes.length).toBe(2);
    }, "pay, ask (back to the gate) and walk away, nothing told to the game");
    g.check(() => expect(g.state("gate_open")).toBe(false), "the dry runs opened nothing");
});
