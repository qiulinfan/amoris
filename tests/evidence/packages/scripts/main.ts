// Three npm packages by name: an ES module (simplex-noise), a package with an ES build and a
// compiler (inkjs/full), and a CommonJS one that requires Node's crypto inside a try (seedrandom).
import { createNoise2D } from "simplex-noise";
import { Compiler } from "inkjs/full";
import seedrandom from "seedrandom";
import { expose, onStart } from "pocket";

const noise = createNoise2D(Math.random);
let line = "";
let picked = "";
let seeded = 0;

onStart(() => {
    const story = new Compiler("A fork in the road.\n* [Go left] You went left.\n* [Go right] You went right.\n- The end.").Compile();
    line = story.Continue() ?? "";
    const choices = story.currentChoices.map((c) => c.text);
    story.ChooseChoiceIndex(1);
    picked = `${choices.join("|")} -> ${story.ContinueMaximally().trim()}`;
    seeded = seedrandom("hello")();
});

expose("noise", () => Number(noise(0.3, 0.7).toFixed(6)));
expose("ink.first", () => line.trim());
expose("ink.picked", () => picked);
expose("seedrandom", () => seeded);
