# Gameplay scenarios

A scenario is a script that plays the game the way a player would and checks what happened: hold an action, wait for something in the world, assert on the exposed state or on events. It runs inside the runtime, in simulated time, headless, at any seed, so "does the first coin still get collected" or "does something ever fall out of the arena" is answered by running the game a hundred times in seconds rather than by watching it once. The intent layer of `docs/design/agent-perception.md`: an agent that changed the game can prove it still works.

```ts
// samples/sprites/scenarios/coins.ts
import { expect, scenario } from "pocket";

scenario("walking right collects the coin ahead within a second", (g) => {
    g.check(() => expect(g.state("score")).toBe(0));
    g.holdWhile("move_x", 1.0);                       // hold the action in the background
    g.until(() => (g.state("score") as number) >= 1, { timeout: 1.0, label: "first coin" });
    g.check(() => expect(g.count("coin.collected")).toBe(1));
});
```

```
pocket scenario sprites                 # every scenarios/*.ts of the project, five seeds each
pocket scenario physics --seeds 20 --only goal --frames 600
```

## Steps

`scenario(name, build)` registers a scenario; `build` receives the tools and lays out steps, which run in order over the ticks (an instantaneous step is followed by the next in the same tick, a waiting step yields until it is done):

- `g.hold(action, seconds, sign?)` presses the action's key through `input.hold` and waits until it is released; `g.holdWhile` presses and continues at once; `g.press(action)` is a one-tick press. All of it goes through the same path as a player's input and into the journal.
- `g.wait(seconds)` lets simulated time pass.
- `g.until(predicate, { timeout, label })` polls every tick and fails after `timeout` seconds (5 by default).
- `g.check(fn, label)` runs code now: `expect(...)` assertions (`toBe`, `toEqual`, `toBeCloseTo`, `toBeGreaterThan`, `toBeLessThan`, `toContain`, ...), spawns, writes, anything the SDK offers.
- `g.state()` is the project's exposed state right now (its `expose()` getters, called directly), `g.state("score")` one value; `g.count(type)` counts events of a type since the scenario started.

The scenario bundle is its own script context (`--scenario <bundle>`) loaded after the project, so its `onStart` sees what the project spawned; one scenario runs per session (`--scenario-name` picks it, the first by default). When it passes or fails it emits `scenario.finished`, and the run quits, so the report carries the state of the deciding tick. The exposed `__scenario` (name, status, step, label, ticks, error) is what the runner reads.

## The runner

`pocket scenario <project> [--file f] [--seeds N] [--frames F] [--only substring]` bundles each `scenarios/*.ts`, runs it once for a frame to list its scenarios, then runs every scenario at seeds 1..N with the frame budget (a scenario still running at the end fails with a note to raise `--frames`), and reports per scenario: seeds run, passed and failed, ticks to pass (min, average, max), and for each failing seed the step it was on and why. The same runner is the `pocket_scenario` MCP tool, and `pocket test` runs the samples' scenarios at three seeds as the `scenarios:<sample>` modules.

Runs are deterministic per seed: a failing seed replays exactly, and `--history` with `recorder.track` or a `transcript` explains it. `tests/evidence/scenarios/` holds the reports for the physics and sprites samples.

## Limits

Scenarios drive input and read state; they cannot pause the simulation or step it themselves (the engine owns the loop). There is no bot with a policy yet: a scenario is a fixed script, so "can the level be finished" needs a script that knows how. Scenarios do not render checks (a capture in a `check` works, but nothing compares images).

## Perception benchmarks

`docs/design/agent-perception.md` promises that an agent can answer gameplay questions in hundreds of text tokens where a frame-by-frame agent needs tens of thousands of image tokens. A perception benchmark is a scenario that measures this: `bench(question, b => ...)` (`sdk/runtime/bench.ts`) plays to the moment of the question with the scenario tools, then `b.measure(label, fn)` answers it through `b.ask(command, params)` calls, each metered (bytes of the request and the reply; tokens at about four bytes each), and `b.verify(truth, compare?)` checks the answer against the omniscient truth (exposed state, a formula) and fails the run when it differs. The record (`__bench` in the exposed state) carries the answer, the truth, the commands used with their bytes, the tokens, the ticks of play the answer needed, and what one image per tick would have cost (`image`, 960x540 by default, at a token per 750 pixels).

```ts
bench("why did the lantern fall", (b) => {
    b.wait(4.5);                                                     // the kick at four seconds snaps the rope
    b.measure("the lantern's last removed component and its causes", () => {
        const lantern = b.ask<number | null>("world.find", { path: "/Lantern" });
        const removed = b.ask<{ events: WorldEvent[] }>("events.since", { seq: 0, type: "component.removed", limit: 20 }).events.filter((e) => e.subject === lantern);
        return b.ask<{ chain: WorldEvent[] }>("events.why", { seq: removed.at(-1)!.seq }).chain.at(-1)!.type;   // the root cause
    });
    b.verify(() => (b.state("ropeIntact") === false ? "joint.broken" : "the rope held"));
});
```

`pocket bench <project> [--file f] [--frames F] [--only substring]` runs every `benches/*.ts` once and prints, per question, the tokens through the instruments, the commands, and the frame-by-frame cost for the same span with the ratio; `--json` gives the rows and totals. `pocket test` runs the samples' benchmarks as `benches:<sample>` (a wrong answer fails the suite, so the instruments' answers are pinned), and `pocket_bench` is the MCP tool. `samples/physics/benches` and `samples/sprites/benches` hold the fixed questions (why did the lantern fall, which body reached the goal first, what the chain carries, is everything inside the arena, how high the jump is, grounded after landing, what the camera sees, where the player is); `tests/evidence/perception/` keeps the measured tables.

The token counts are estimates (bytes over four), the image cost is a formula, and the strategies are fixed scripts rather than a model choosing its own instruments: the benchmark measures the instruments, not an agent. Running models against the questions with only the SDK types and the docs is the agent eval suite, which is not built.
