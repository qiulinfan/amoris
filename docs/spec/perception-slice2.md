# Perception and projections: slice 2 decisions

Status: Draft, slice 2

The decisions slice 2's implementation of perception and its projections (the modules `perception`
and `projection` of `pocket-interface`, and the skipper's declarations in
`samples/sailing-course/perception.json`) took where [perception.md](../../shared/contract/perception.md),
[projection.md](../../shared/contract/projection.md) and
[sailing.md](../../shared/contract/sailing.md) (The skipper's perception) were ambiguous or wrong
for the code. Those files are the shared contract, which changes on both lines first and is pinned
by `shared/SYNC.toml` (charter 6.2; checks.md 5.4), so this line records its choices here, numbered,
until the contract takes them or replaces them.

## 1. Where the decisions live

`shared/contract/*.md` and every file under `shared/` are hashed in `shared/SYNC.toml`; a changed or
added file fails `gen.shared_modified`. Slice 2 therefore records the contract's open choices here,
as the slice 1 decisions sit beside the specifications they amend.

## 2. World state as built

`Observer` is `{profile, seat: Option<String>, omniscient: bool}`: a seat id is a string (README,
Seats and callers), and the binding to the reserved `omniscient_player` profile is a flag beside the
profile it is derived from, so a fork and a replay carry the benchmark's condition with the world.
Binding or unbinding (`perception::bind_omniscient_player`, a boundary write) clears the memory and
the ring's entries, keeping the ring's count, so nothing known under one binding is carried into the
other. The profile name `omniscient_player` is refused in a game's declarations.

`MemoryEntry` also keeps the `detect_m`, `height_m` and `priority` the entity had when last seen:
ranking and the forgetting rule (step 6) then read only what the observer saw, never the live
entity, whose values may have changed unseen. A ring entry is the struct `RingEntry {world, event}`,
the world event's `EventSeq` (none for a sighting) beside the perceived event. `FactValue`'s
persisted form is externally tagged with a position as `[x, y, z]`; answers write the contract's
untagged wire form. `Occluder` is an empty struct (`{}`) so its persisted and JSON forms are an
object.

`ObserverMemory` also keeps `updated`, the tick of the update that wrote it, and an entry is `Seen`
when its `seen_tick` is `updated` (its age counts from `updated` too), not when it is the clock's
tick. At a boundary the two are the same; inside a tick the clock is one ahead of the last update,
and comparing with the clock made every entity an executor or an NPC rule looked at in `Control`
read as remembered, so a rule asking whether it sees the player never did. Through `PerceptionView`
inside a tick an observer therefore sees what the last update saw, at the end of the previous tick.

These components derive `Component` and `Resource`, and the memory and the ring hold enums below
their top level that `Persisted::trace` traces first, so `pocket-interface` takes `bevy_ecs` and
`serde-reflection` (architecture.md 6, both rows). `perception::declare` names them for persistence;
the declarations (`PerceptionDefs`) are ignored game data, installed with the game.

## 3. Declarations

- `FactSource` has a third form, `{"from": "data", "path"}`: an event's declared data field reads a
  member of the event's data by a dotted path, which neither `field` nor `derived` names. Only event
  data fields take it, and they take only it.
- Validation (`definition.invalid` at the first fault, with its JSON pointer): names are snake_case
  and unique per list; a unit's suffix matches (`_m`, `_mps`, `_s`, `_kg`, `_deg`); event kinds are
  dotted; ranges, memory, budgets (1 to 16000) and fields of view (0 to 360] are finite and sane;
  every sighting kind, instrument and derived function named exists; a tensor layout names declared
  instruments, kinds and facts.
- An event's data field is `chart`, `coarse` or `hidden`, and not `relative` (`definition.invalid`
  at its `exposure` or `relative` member otherwise). Whoever perceives an event gets its data:
  `full` would need an attention the event's scope does not measure and `owner` an owner it does not
  name, and the data is the same for every observer, so nothing in it is relative. Before this rule
  a data field declared `owner` reached every observer that perceived the event, and a `full` one
  observers beyond their attention.
- Derived facts and instruments are Rust functions registered by name (`PerceptionDefs::derive`);
  the sailing ones are `perception::sailing`. Script functions (`pocket_script::call_named`, which
  takes `{entity, observer}`) are not wired: a relative fact of a remembered or charted entity must
  be computed from the position the observer knows and the facts it remembers, which such a call
  does not carry, so it could read the live entity. They wait for a call that carries `at_m` and the
  known facts, and for `pocket-runtime` to hand perception a hook (perception cannot link
  `pocket-script`, architecture.md 5).

## 4. The perception update

- `PerceivedEvent.tick` is the world event's own tick: an event a boundary wrote after tick T has
  tick T, though the update of T + 1 perceives it. A sighting has the update's tick.
- Attention (step 5) is measured from the eye in three dimensions, as range is. Sound (step 7) is
  measured from the eye, or from the body's origin for an observer without sight.
- The forgetting test of step 6 applies step 4 to the remembered origin and its top (the remembered
  `height_m` above it), as for a candidate, rather than to the origin alone.
- A `Sight` event's occlusion test excludes the event's subject and the observer's body from the
  occluders, as step 4 excludes the target, so an event on an island is not hidden by the island.
- An event's subject is shown when the observer knows it after this tick's memory update: its own
  body, an entity it remembers or sees, or a charted entity when the profile reads the chart. A
  remembered subject goes by the name it had when last seen, as its percept does, so renaming an
  entity out of sight changes no event a player perceives.
- A perceived event's `bearing_deg` and `range_m` are shown only where the observer perceives the
  place: a `sight` or `sound` event (it saw or heard it there), a `private` one (on its own body),
  and a `global` one only when the place is its subject's position (the event has no `at_m` member)
  and the subject is the observer's body or an entity it sees this tick. Otherwise both are `None`.
  Step 7 admits a global event wherever it happened; giving its place from the subject's live
  position told every player where an unseen entity was (a hidden move changed a player's answer),
  and an `at_m` member is not a declared data field. A game that wants every observer to know where
  declares the place as a data field (unit `position`).
- The capacity (step 6) bounds the entries remembered unseen: an entity seen this tick is never
  evicted, so a visible set larger than `memory_capacity` is kept whole, and the oldest unseen
  entries (then the lowest `EntityId`) go until the rest fit or only seen ones remain. Evicting a
  seen entity made it a first sighting again on the next tick, a `sighted` event (a decision point,
  sailing.md, Time) every tick that flooded the ring.

## 5. Queries and answers

- The raw omniscient view has no observer: bearings and ranges are taken from the named seat's body
  when the request names one, else from the world's origin. Its events are those the world still
  holds (the inbox: the last tick's and the boundary's since), numbered by `EventSeq`, an undeclared
  kind's data shown member by member unrounded. Its default budget is 2000 tokens, as it has no
  profile to take one from. A non-`Perceivable` entity's facts are the top-level fields of the
  engine components this crate can read (by their serde form) and every field of its project
  components (by their registered schema).
- An entity a caller does not know is answered exactly as one that does not exist
  (`perception.unknown_entity`); name suggestions come only from the names it knows; `Name#id` whose
  name is not the known one is unknown.
- Filling (Token budgets, step 4) takes the longest prefix whose answer fits, and the query is
  refused with `perception.budget_too_small` only when no prefix fits, `detail.min_tokens` being the
  tokens of the smallest answer over every prefix. An item line can be shorter than the omitted
  line, with its hint, that replaces it, so an answer's size need not grow with its prefix, and
  testing only the empty prefix refused answers that fit. A larger budget still answers a superset,
  the set of fitting prefixes only growing with it.
- The push delta (`fill::delta`) takes the longest prefix of events that fits its byte limit; when
  not even the text's omitted line fits, it writes nothing and its cursor stays at `since`, so a
  delta never exceeds the share of the answer's budget it was given.
- `nearby` and `events` count what their `limit` cut off in `omitted`, as the budget's cut.
  `describe` chooses its last five events newest first under the budget and writes them oldest
  first; it has no omitted section (the grammar's `describe` has none).
- The JSON projection is written member by member (not through `serde`'s derive), so each value is
  rounded where it is written and the parts other layers render (an active intent, a pending
  decision) are spliced in as they wrote them. `t_s` has three decimals in JSON and one in the text
  header; `pos_m` has one decimal and appears in JSON only (the text grammar has no position).
- A tensor answer is given for `observe` only; `nearby`, `describe`, `events` and the raw omniscient
  view refuse `projection: "tensor"` with `request.not_applicable`. A `Rays` ray meets the
  perceivable entities the observer sees now and the occluders that are not perceivable (terrain);
  it passes through what the observer does not see, so it tells nothing a percept does not, and its
  distance column is the hit distance over the block's `range_m`. A `Nearest` row's range column is
  over the profile's sight range (1 for a blind profile); a text fact takes no column.
- `Observation`, `write_observation`, `write_nearby`, `write_describe` and `write_events` write an
  answer from parts, so the golden cases and other layers can project what they build without a
  world.

## 6. The golden cases

projection.md's `shared/contract/conformance/projection.jsonl` does not exist and cannot be added
here (1). The cases are Rust tests (`crates/pocket-interface/tests/perception_golden.rs`) built from
the sailing sample's declarations. A case cannot be a JSON answer alone, as projection.md proposes:
the JSON drops the precision the text needs (`"hoist": 1.0` is `hoist=1.00` in text), so a shared
case must carry its declarations too. sailing.md's example observation is reproduced byte for byte:
774 bytes, 194 tokens, and at 185 tokens the first three candidates with their omitted line in 739.

## 7. The checks as built

- `contract.perception.read_only`, `deterministic`, `budget` and the visibility unit tests run on
  the sailing probe (`perception::probe`: the skipper, an island hiding a crate, charted islets and
  marks, crates near and far, a second boat) and on a small watcher world, with world hashes, forks
  and restores through `pocket_physics::probe`'s ledger with perception's types added, since
  `pocket-interface` cannot link `pocket-persist` (architecture.md 5).
- `contract.perception.noninterference` covers perception's part: its queries in every projection
  (refusals among them) and six mutation classes (a hidden fact, an entity outside every range moved
  within that region, a crate behind an island respawned behind it, a hidden-scope event, an
  undeclared kind, a charted unseen mark moved). Before each round's ticks the session's world emits
  the same events in both runs about the entities the mutations touch: a global and a sound event
  about the far crate, a sight event about the crate behind the island, a global event about the
  unseen charted mark and a global event with an `at_m` member, so what a player perceives of an
  event about an unseen entity is compared too. The 256 cases are drawn from a seeded PCG32 rather
  than `proptest`, which would add an external crate for one test; a failure prints its seed. A
  negative control checks that a perceivable change does change the answers. The session tools and
  mutation classes of other layers (`act`, `step` with `until`, `intents`, a requested decision,
  another seat's clock) are checked with those layers.
- `contract.perception.budget` sweeps `observe`, `nearby`, `events` and `describe` in text and JSON
  from the smallest budget that answers to 2000 in steps of 10, and the push delta's byte limit:
  every answer within its budget, its items a prefix of the whole answer's (the newest events for
  `describe`) and never fewer at a larger budget, the refusal below the smallest naming it as
  `min_tokens`, and a text answer given again, byte for byte, at its own size in tokens. A world
  with one short event, whose line is shorter than the omitted line, checks that the smallest budget
  is that answer's own size.
- Native and WebAssembly answers are compared through `examples/web_perception.rs`, a module with no
  imports run by `tests/web/run.mjs` in Node or by `tests/web/index.html` in headless Chrome
  (`tools/webcheck.py`), against the report the `perception_web` test writes: every query kind every
  60 ticks for 600 ticks and a fork's answers beside the original's for 120 ticks. It is not yet a
  step of `cargo xtask check`.

## 8. The sailing skipper

- The declarations name project components the game declares: `Tally {taken, total}` (the sample's),
  and `Mark {color, round_to, order}` and `Course {next}` for a course, which the sample does not
  declare yet; until a course is laid, marks show no facts and `next_mark` reads `none`.
- `Course.next` is the `order` of the mark to round next, as `mark.rounded`'s `order` data is, never
  an entity id: `next_mark` names the lowest-id entity with a `Mark` whose `order` equals it (or
  `none`), and a mark's `next` fact is whether its own `order` equals it. Matching ids as well made
  any entity whose id equalled `next` (the sea, an island) the next mark, and flagged a mark whose
  id equalled another's order.
- Instruments as computed: `course_deg` from the boat's velocity over the ground; `wind_from_deg`
  and `wind_mps` from the wind at the boat (gusts included); `twa_deg` from the true wind and the
  heading; `left` is `total - taken`; `hoist`, `sheet` and `rudder` read the actuators' current
  values (`hoist_now`, `sheet_now`, `rudder_now`), not their targets.
- Facts as computed: a boat's `sail` is `set` when its hoist is at least 0.5; an island's `radius_m`
  is its collider's horizontal reach from its centre; `shore_m` is the distance to the centre less
  the known radius, and `shore_brg_deg` the bearing of the centre; `alongside` holds within 3 m
  across the water and 3 m up or down.
- `perception::sailing` gives each kind's `Perceivable` from the kinds table (`island`, `mark`,
  `boat`, `adrift`) and the skipper's `Observer`, for the scene loader to put on the sample's
  entities. Wiring the plugin and these components into the runtime's sailing scene is the
  runtime's.

## 9. First measurements

`cargo run -p pocket-interface --example perception_bench --release` on the development machine,
with the owner's other jobs running beside it (so for reading, not for the reference figures, which
`cargo xtask perf --set-unset` sets from a calibrated run by budgets.md's rule):
`interface.perception` on the sailing probe 22 to 26 µs a tick (`perception.tick`); with 100 NPC
observers over 1,000 perceivable entities among 20 occluding walls 3.5 ms a tick (`perception.npc`;
7.1 ms before the candidates were gathered once a tick for every observer rather than once per
observer); `observe` at the skipper's default budget in text 100 to 127 µs (`observe.sail`). The NPC
case was not profiled: the rays and the copy of each observer's memory and ring every tick are the
likely costs, and a spatial index (step 1) and in-place updates the next steps if a game needs more.
