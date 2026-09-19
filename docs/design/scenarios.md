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
- `g.bot(name, policy, options)` and `g.stopBot(name)` start and stop a bot (below); `g.report(key, value)` records a number or a structure for the run's report.

The scenario bundle is its own script context (`--scenario <bundle>`) loaded after the project, so its `onStart` sees what the project spawned; one scenario runs per session (`--scenario-name` picks it, the first by default). When it passes or fails it emits `scenario.finished`, and the run quits, so the report carries the state of the deciding tick. The exposed `__scenario` (name, status, step, label, ticks, error) is what the runner reads.

## The runner

`pocket scenario <project> [--file f] [--seeds N] [--frames F] [--only substring]` bundles each `scenarios/*.ts`, runs it once for a frame to list its scenarios, then runs every scenario at seeds 1..N with the frame budget (a scenario still running at the end fails with a note to raise `--frames`), and reports per scenario: seeds run, passed and failed, ticks to pass (min, average, max), and for each failing seed the step it was on and why. The same runner is the `pocket_scenario` MCP tool, and `pocket test` runs the samples' scenarios at three seeds as the `scenarios:<sample>` modules.

Runs are deterministic per seed: a failing seed replays exactly, and `--history` with `recorder.track` or a `transcript` explains it. `tests/evidence/scenarios/` holds the reports for the physics and sprites samples.

## Bots

A bot is a policy that plays: `g.bot(name, policy, { every, seconds })` starts it as a step, and from then on the policy runs every tick (or every `every` ticks) until the scenario ends, `seconds` pass, `g.stopBot(name)` runs, or the policy calls `view.stop()`. The policy sees a `BotView`: the tick and times, the exposed state (`view.state("player.x")`), a `random()` stream of its own seeded from the run's seed and the bot's name (so a bot never disturbs the game's randomness and replays exactly per seed), `memory` it keeps between ticks, and two verbs: `hold(action, sign?)` keeps an action's key down for this tick (call it every tick the key should stay down; the hold is refreshed under the key, so there is no new press edge), and `press(action)` presses for one tick. Everything goes through `input.hold` / `input.press` like a player's keys, so the journal and the replay carry what the bot did.

```ts
g.bot("runner", (v) => {
    v.hold("move_x");                                                    // always run right
    const m = v.memory as { best?: number; since?: number };
    const x = v.state<number>("player.x");
    if (m.best === undefined || x > m.best + 0.05) { m.best = x; m.since = v.tick; }
    if (v.state<boolean>("player.grounded") && v.tick - (m.since ?? v.tick) > 15) { v.press("jump"); m.since = v.tick; }   // stuck: jump
});
g.until(() => g.state<number>("player.x") > 8.5, { timeout: 15, label: "the east edge" });
```

`bots.random({ holds, presses, hold: [min, max], pressChance, idleChance })` is a fuzzer: it holds a random action (or nothing) for a random stretch and presses at random, all from its stream, so ten seconds of mashing at five seeds is a cheap check that nothing leaves the level, crashes or soft-locks. The run's `bots` (ticks run, holds started, presses) are in `__scenario` and in the runner's rows.

## Analyzers

`analyze` (`sdk/runtime/analyze.ts`) turns recorded motion into numbers. `analyze.series(entity, component, field)` reads one field over the ticks from the frame recorder (`recorder.start(ticks)` first; booleans become 0 and 1), and the pure functions work on any series: `summarize` (extremes with their ticks, mean, spread, `delta`, and `settledAt`, the tick from which the value stayed within a tolerance of its last), `segments` (the series cut into rising, flat and falling stretches with their change), `peaks` (local maxima with their prominence), `rate` (change per second), `pathLength` (distance travelled over one to three coordinate series) and `jumps` (from a height and a grounded series: liftoff, landing, apex and airtime of every stretch in the air, a drop off a ledge included). `g.report(key, value)` puts a number or a structure into the run's report, which the runner prints for the first passing seed (`report (seed 1): coins=3, jumps=4, apex=1.86`) and keeps in the rows; the physics sample reports when the marble's height settled and how many times it crested, the sprites bots report the jumps' apex and the distance a fuzzer covered.

## Limits

Scenarios drive input and read state; they cannot pause the simulation or step it themselves (the engine owns the loop). A bot's policy is script: there is no learned or search-based player inside the runtime; a program that plays step by step uses the environment interface instead (`docs/design/environment.md`). Scenarios do not render checks (a capture in a `check` works, but nothing compares images).

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
