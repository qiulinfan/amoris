import { dialogue, events } from "pocket";
import { expect, test } from "pocket/test";

const guard = {
    vars: { gold: 7, name: "traveller" },
    start: "gate",
    nodes: {
        gate: [
            { say: "Guard", text: "Halt, {name}! The toll is 5 gold." },
            {
                choice: [
                    { text: "Pay ({gold} gold left)", if: "gold >= 5", set: { gold: "gold - 5", paid: true }, goto: "paid" },
                    { text: "Bribe with a song", if: "bard", goto: "song" },
                    { text: "Turn back", goto: "end" },
                ],
            },
        ],
        paid: [
            { if: "gold < 5", then: [{ say: "Guard", text: "Only {gold} left. Spend it well." }], else: [{ say: "Guard", text: "Rich, are we?" }] },
            { event: "gate.open", data: { by: "toll" } },
            { text: "The gate swings open." },
        ],
        song: [{ say: "Guard", text: "Lovely." }],
    },
};

test("a conversation runs its lines, offers the choices whose conditions hold, sets and branches", () => {
    const seq = events.lastSeq();
    const c = dialogue.start(guard);
    expect(c.line).toEqual({ speaker: "Guard", text: "Halt, traveller! The toll is 5 gold." });
    c.next();
    // The song needs a bard: two choices open, the first with the gold filled in.
    expect(c.choices.length).toBe(2);
    expect(c.choices[0].text).toBe("Pay (7 gold left)");
    c.next();   // waits for a choice
    expect(c.choices.length).toBe(2);
    c.choose(0);
    expect(c.vars.gold).toBe(2);
    expect(c.vars.paid).toBe(true);
    expect(c.line!.text).toBe("Only 2 left. Spend it well.");
    c.next();
    expect(c.line).toEqual({ speaker: "", text: "The gate swings open." });
    c.next();
    expect(c.done).toBe(true);
    const types = events.since(seq).map((e) => e.type);
    expect(types).toContain("dialogue.line");
    expect(types).toContain("dialogue.choice");
    expect(types).toContain("gate.open");
    expect(types[types.length - 1]).toBe("dialogue.end");
});

test("variables given at the start open other choices; a missing node is named", () => {
    const c = dialogue.start(guard, { gold: 1, bard: true });
    c.next();
    expect(c.choices.map((x) => x.text)).toEqual(["Bribe with a song", "Turn back"]);
    c.choose(1);
    expect(c.done).toBe(true);
    expect(() => dialogue.start({ start: "nowhere", nodes: { a: [{ text: "hi" }] } })).toThrow();
});

test("check finds missing nodes, broken expressions, unknown steps and nodes nothing reaches", () => {
    expect(dialogue.check(guard)).toEqual([]);
    const problems = dialogue.check({
        start: "a",
        nodes: {
            a: [
                { say: "X", text: "You have {gold +} gold" },
                { if: "gold >> 5", then: [{ goto: "nowhere" }] },
                { choice: [{ text: "Go", goto: "b", set: { gold: "gold @ 2" } }] },
            ],
            b: [{ shout: "hey" } as never],
            lost: [{ text: "Nobody hears this." }],
        },
    });
    const where = problems.map((p) => `${p.node}/${p.step}`);
    expect(where).toEqual(["a/0", "a/1", "a/1.then.0", "a/2", "b/0", "lost/"]);
    expect(problems[2].problem).toContain("'nowhere'");
    expect(problems[5].problem).toBe("no goto reaches it");
});
