# Agent perception: the observable world

- Status: working design note (2026-09-17). Feeds ADR 0003 (world model) and later records. Not a
  decision yet.

## The problem

An agent that builds a game must know whether the game does what was intended, what is happening in
the world, why it happened, and whether it feels right. Today an external agent has two instruments:
the source code and screenshots. Screenshots are the player's eyes, not the developer's: they show
appearance, not velocity, contacts, cooldowns, state machines or causes; they are slow, lossy and
expensive; and a sequence of them still has no notion of time or causality. Hand-written probe
functions ("did the hit land?") are the agent rebuilding, badly and per project, the instruments a
developer takes for granted.

Human developers do not debug gameplay by staring at frames either. They use debug draws, logs,
breakpoints, profilers, replays and a mental model of the simulation. The design goal is to give
agents those instruments natively, structured, queryable, and designed in from the first milestone.
Vision becomes the last resort, used for aesthetics with better tools, not the first resort for
everything.

Precedents: web agents became reliable when they got the accessibility tree instead of pixels.
Voyager mastered Minecraft through Mineflayer's structured observations and actions, not through
frames. Unity ML-Agents observed the world through sensors (vectors, raycasts) long before it used
cameras. Games need their accessibility tree.

## What "perceiving the game" decomposes into

| Layer | Question | Instrument |
|---|---|---|
| State | What is the world right now? | The world as data: every entity and component queryable, snapshots, diffs, natural-language descriptions. |
| Causality | What happened, and why? | An event bus where every event carries time, source entity and the id of the event that caused it; "why" is a walk up the chain. |
| Space and shape | Where is it, what shape, can it be reached or seen? | Geometric queries (ray, overlap, path, line of sight), asset lint (bounds, scale, pivot, materials), a per-pixel entity id buffer from the renderer. |
| Time and feel | How does it change over time; does it feel right? | A frame recorder with time travel, and gameplay analyzers that turn motion into numbers (jump profile, input latency, frame pacing, damage per minute). |
| Vision | How does it look? | Targeted captures: named cameras, debug render modes, contact sheets, image diffs, plus the id buffer so the agent knows what is in the picture without guessing. |
| Intent | What should happen? | Executable gameplay specs: scenarios with bots and assertions, run headless at many seeds and high speed. |
| Embodiment | What is it like to play? | An environment interface (observe, act) with structured observations, so bots and agents can play, and human play sessions can be recorded and inspected. |

## The preconditions, and why they cannot be retrofitted

1. **The world is data.** In an ECS every piece of state that matters is a component in a table,
   uniformly serializable and queryable. Scripts keep their state in components, not in closures;
   the API makes that the natural way. Object trees with state inside script members (Unity, Godot)
   hide most of the world from any observer.
2. **Determinism and virtual time.** Fixed step, seeded randomness, no wall clock in simulation
   code, deterministic physics on a given machine. A run is reproducible from its inputs and seed,
   which makes replays tiny (inputs, not frames) and lets the agent step, pause, fast-forward at
   100x, and rerun with one variable changed.
3. **A causal event bus.** Physics contacts and triggers, spawns, damage, state transitions, input,
   script-emitted events, all with cause ids. Without cause ids an event log is a list; with them it
   is an explanation.
4. **A frame recorder.** Per-frame component deltas and events in a ring buffer, cheap because ECS
   storage makes deltas contiguous. It gives time travel: query the world at time t, diff t1 and t2,
   find the first frame where a predicate holds.
5. **A renderer that reports.** An entity id buffer and depth are nearly free outputs of the render
   pass. From them: which entities are visible, where, how large, and what occludes what. Debug
   modes (wireframe, colliders, normals, overdraw, lighting only) are the same pipeline with a
   switch.
6. **Operability.** Every instrument is reachable from the CLI (JSON) and from MCP, headless, and
   shows the same data to the human in the editor (timeline scrubber, query panel, why panel).

A scripting language change does not provide any of these. This is the answer to "why Pocket3D and
not Godot": Godot was built for a human looking at the screen; Pocket3D is built for an agent that
must understand the world without looking, and for a human who directs it.

## What it looks like

State query and description (TypeScript and MCP share the same surface):

```ts
const near = world.query({ with: [Enemy, Transform], within: { of: player, radius: 20 } });
world.describe(player);
// "Player at (3.0, 0.0, 12.1), standing on Ground, facing +Z, health 40/100,
//  4 enemies within 20 m (nearest Grunt#12 at 6.2 m, state Patrol)"
```

Causality:

```ts
const death = events.last({ type: "Died", entity: player });
events.why(death);
// Died(Player) <- Damage(30, from Bullet#88) <- Fired(Turret#3, rate 0.13s)
//   <- TriggerEnter(Player, TurretZone#3) at t=7.92
```

Time travel and physics explanation:

```ts
const t = recorder.firstFrame(f => f.get(Crate#7, Transform).y < 0);
physics.explain(Crate#7, t);
// "Tunneling: velocity 41 m/s, floor thickness 0.2 m, one substep at 1/60 s. Suggest CCD or 4 substeps."
```

Gameplay analyzers and specs:

```ts
metrics.jumpProfile(player);           // apex 2.1 m, airtime 0.82 s, input-to-liftoff 1 frame
scenario("first platform", async (g) => {
  g.press("forward", 2.0); g.tap("jump");
  await g.until(f => f.on(player, PlatformB), { timeout: 4 });
}).seeds(100).speed(100);              // 100 runs, 100x realtime, JSON report
```

Visibility without vision:

```ts
render.visible(camera);                 // [{ entity: Boss#1, coverage: 0.11, center: [0.52, 0.40] }, ...]
render.capture(camera, { mode: "colliders", contactSheet: { frames: 8, span: 1.0 } });
```

## Two axes: the tree and the timeline

Two ideas from the project owner sharpen the model above.

**The hierarchy tree itself must be AI-native.** Humans and language models both think about a scene
as a tree: nodes with children and components, addressed by paths. The ECS is the storage and query
substrate; the tree is the document. In an ECS the parent relationship is itself data, so the tree
is a complete view: every piece of state is a component on a node, nothing hides in script members.
AI-native then means:

- One canonical text form, used for the scene file, for the runtime snapshot, for the editor's
  hierarchy panel and for MCP: one node per line,
  `name#id [components] {salient properties} {derived annotations}`, children indented, stable ids
  and paths such as `/World/Arena/Turret#3`.
- Level of detail on demand. A forest of two thousand instances reads as one line with a count and
  bounds until the agent zooms in. Every observation call takes a token budget and returns the most
  salient content within it.
- Derived annotations on nodes: `grounded`, `visible 11%`, `state Patrol`, `nearest Player 6.2 m`,
  anomalies. The tree carries meaning, not only raw numbers.
- Diffs as the unit of change: agents read and write tree diffs, never whole dumps.
- Addressability: every line has a handle the agent can act on (query it, capture it, edit it, ask
  why).

**Perception is compression of signals into stable states.** A human does not know the character is
grounded by watching frames; the vertical position stops falling and settles into a small
oscillation, and that stability is the perception. The useful information in a second of play is a
short sequence of numbers, not sixty images. The engine therefore turns raw series into segments and
events:

1. Segment time at change points of derived predicates and of raw series (falling, landing,
   grounded, moving, stuck, oscillating). Event segmentation theory in cognitive science describes
   human perception the same way: boundaries appear where prediction fails.
2. Summarize each segment with a few statistics (duration, displacement, mean velocity, extrema) and
   attach the events that happened inside it, with their causes.
3. Rank by salience: what deviates from siblings, from history or from declared design values (a
   turret firing six times faster than the others). The agent is told what is surprising first.
4. Keep the raw data reachable: each segment links to frames, entities and events, so the agent can
   zoom from narrative to numbers to pixels.

The result is a **gameplay transcript**: the compact, semantically rich, drill-down-able record of a
session.

```
0.00-1.20  Player falling, y 8.0 -> 0.0 (spawn drop)
1.20       Landed on Floor, impact 12 m/s
1.25-5.00  Player grounded, moving +x at 4 m/s
5.00       Jump: apex 2.1 m at 5.41, airtime 0.82 s
7.92       TriggerEnter Player -> TurretZone#3
7.92-8.30  Turret#3 fires x3, interval 0.13 s        salient: 6x faster than Turret#1, #2
8.30       Player died <- Damage 30 x3 from Bullet#86, #87, #88
```

Predicates come from three sources: engine built-ins (grounded, visible, contacts, triggers,
animation state, health changes), game-defined derived signals declared in components by scripts,
and a generic change-point analyzer that segments any numeric series without game-specific code.
Predicates are always defined and inspectable; the engine never guesses semantics it cannot show.

Memory is multi-resolution, as in people: the last seconds at full fidelity in the recorder, older
history as segments, the whole session as a transcript.

**A benchmark for perception efficiency.** Alongside the agent eval suite, measure tokens consumed
and correctness on a fixed set of gameplay questions ("why did the player die", "is the jump
grounded on landing", "what does the camera see at the boss intro"). A frame-by-frame agent needs
tens of thousands of image tokens; the target here is hundreds of text tokens with a linkable
answer.

## Adopted from the September 2026 survey

`docs/research/agent-native-survey.md` checked the known engines and the research record. Nobody
ships the layers above; the fragments that exist and the research results fixed several details of
this design: the protocol shape (JSON-RPC with schema, discovery and watch streams, typed commands
plus an `eval` escape hatch, batch and transactions, permission levels with reversible checkpoints),
observation tiers (player-knowable versus omniscient), step-gated simulation control with the model
never inside the tick, a Trace-like self-describing event log with provenance and cause ids, the
determinism package (input journal, per-tick state hash, first-divergent-tick diagnosis,
exponentially spaced snapshots), a Rerun-compatible data layer, Bayesian online change-point
segmentation, and a perception benchmark against external baselines.

## Limits, stated plainly

- Aesthetic judgment still needs vision; the instruments make it cheaper (fewer, better images with
  known content), not unnecessary.
- "Feel" is partly subjective. Analyzers give numbers and references; a human still decides.
- Recording costs memory and some time. Ring buffers, deltas and sampling keep it in budget; shipped
  builds turn it off.
- Determinism is engineering discipline: virtual time, seeded randomness, no wall clock, careful
  physics. Same-machine determinism is the requirement; cross-machine bit-exactness is a stretch
  goal.
- State that scripts keep outside components is invisible to queries. The API and the eval suite
  steer state into components; the V8 inspector remains available for the rest.

## Where it lands in the roadmap

- M0: determinism and virtual time, headless stepping, speed control, structured logs.
- M1: the world as data, the canonical tree text form with level of detail and token budgets,
  snapshots and diffs, query API, `describe`, the causal event bus.
- M2: id buffer and depth outputs, debug render modes, targeted captures, contact sheets, image
  diffs.
- M3: the recorder and time travel, segmentation and the gameplay transcript, exposed in the editor
  as a timeline and a why panel, and over MCP.
- M4: physics explanation, gameplay analyzers, executable scenarios with bots.
- The environment interface for play bots and learned players: `docs/design/environment.md`
  (`env.reset/step/observe`, the Python client). Recorded human sessions are journals (`--record`),
  replayable with `--replay`.

## Implementation pointers (2026-09-19)

What this document asks for and where it now lives:

- State: the tree with budgets and salience is `world.tree`, with `world.describe`, `world.query`,
  `world.summary` (`engine/world/src/world.cpp`).
- Causality: the event log is `engine/world/events.hpp`; `events.why {seq}` walks the cause links
  and returns the chain with a one-line story
  (`component.removed(/Lantern) at tick 241 <- joint.broken(/Lantern) at tick 241`), and says
  whether the chain is complete or stopped at an evicted event.
- Space and shape: `physics.raycast`, `physics.overlap`, `physics.contacts`, `physics.joints`;
  `nav.path` and `nav.reachable` over a baked walkability grid answer "can it get there"
  (`docs/design/navigation.md`); the id buffer is the renderer's second target, read by
  `render.pick` (one pixel), `render.ids` (every entity with pixel count, bounds and center) and
  `render.visible` (the same, largest first, without the background: what the camera sees, as
  numbers).
- Time: `--history N` or `recorder.start {ticks}` keeps the last N ticks of every entity's
  components as deltas (`engine/world/recorder.hpp`); `recorder.at {tick}` replays the world or one
  entity at a tick, `recorder.diff {from, to}` lists what spawned, died, was renamed and which
  fields changed, `recorder.track {entity, component, field}` returns a field over time,
  `recorder.first {entity, component, field, op, value}` finds the first tick a predicate holds.
  Change detection hashes components and converts only what changed to JSON; the sanitized debug
  build pays about 1.6 ms per tick for the physics sample (31 entities, a dozen moving) and about
  250 ms for the swarm (3000 entities moving every tick), so recording is off by default and on
  demand. The gameplay transcript with segmentation is `engine/world/transcript.hpp`.
- Determinism: `World::hash` and the session's per-tick state hash, `--record/--replay` for input
  journals.
- Vision: `capture` and `render.ids {path}` write PNGs of the frame and of the id buffer;
  `render.debug {colliders, joints, bounds, axes}` and `debug.line/box/sphere` draw the invisible
  (collision shapes, joints, bounds, a script's own markers) as lines into the same captures
  (`docs/design/rendering.md`).
- Operability: everything above is the runtime's command set, served over HTTP and MCP
  (`docs/mcp.md`), from the SDK (`events.why`, `recorder.*`, `render.visible`) and in the editor's
  panels.

- Intent: executable gameplay scenarios (`docs/design/scenarios.md`): `scenario(name, g => ...)`
  plays through actions, waits and checks in simulated time; `pocket scenario <project> --seeds N`
  runs them at many seeds and reports.
- The perception benchmark: `bench(question, b => ...)` answers a fixed gameplay question through
  metered runtime commands and verifies it against the omniscient truth; `pocket bench <project>`
  reports tokens against the frame-by-frame cost (`docs/design/scenarios.md`, Perception benchmarks;
  measured tables in `tests/evidence/perception/`). On the two samples the eight questions cost 46
  to 1493 tokens each, 170 to 1600 times less than one image per tick would.

Not built: observation tiers (player-knowable versus omniscient), physics explanation, gameplay
analyzers beyond the transcript, scenario bots with policies, and the agent eval suite (models given
only the SDK types and the docs, scored on the same questions).
