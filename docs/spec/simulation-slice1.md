# Simulation: slice 1 decisions

Status: Draft, slice 1

The decisions slice 1's implementation (the crate `pocket-sim`, and `pocket-physics` for 16) took
where [simulation.md](simulation.md) was ambiguous or wrong for the code. They are open choices 7 to
16 of simulation.md 14, which lists them by title and keeps their numbers here; their measurements
are in simulation.md 13, their error codes in simulation.md 11.

## 7. The schedule is a list `Sim` runs, not a `bevy_ecs` `Schedule`

`pocket_sim::Sim` keeps its systems sorted by their row of simulation.md 4.4 and runs them one by
one with `System::run`, applying each system's deferred commands when it returns. Reason: the step
needs a point between systems to call `StepHooks::system`, to catch a panic and name the system it
came from (`sim.internal {tick, phase, system, message}`), and to stop and poison the world there; a
`Schedule` of chained sets gives none of these. The order is total by construction, so ambiguity
detection has nothing to find. `StepHooks::phase` is called for all six phases every tick, empty or
not.

A system is a `bevy_ecs` system (`Sim::add_system`) or a function over the whole world with a
`SystemCtx` (`Sim::add_exclusive`, what `script.update` uses: the context carries the step's hooks,
so the host times each script system, and `fault` for a fault that poisons). A `bevy_ecs` system
whose parameters skip it (a `Single` or `Populated` with nothing to match,
`RunSystemError::Skipped`, which `bevy_ecs` documents as not an error) did not run that tick; any
other run error is `sim.internal`. Neither form keeps state across ticks outside the world
(simulation.md 4.1, invariant 4): `bevy_ecs::system::Local` is on the disallowed list of
simulation.md 10, and an exclusive closure may keep only what is rebuilt identically from the world
(the script host's stateless VM and compiled code).

## 8. System keys place systems

A crate registers a system under its key and phase. A key of simulation.md 4.4's table takes that
row (in another phase it is refused, `sim.system_phase`); any other key runs after the listed
systems of its phase, before `sim.finish` in `Finish`, in registration order.
`Sim::schedule_listing` prints the flattened schedule (`phase key condition` per line) for the
schedule golden; pocket-sim's own test pins it for a schedule with every row filled
(`crates/pocket-sim/tests/golden/schedule.txt`), the runtime's pins the real one.

## 9. The tick's bookkeeping is Derived resources

`TickState` (the running tick and the open invocation), `TickOutput` (failed invocations and
decision requests, which systems write through `ResMut<TickOutput>`), `EventOutbox`, `RngTable`,
`EntityIndex`, `ReservedIds` (choice 11) and `Poisoned` are resources of class Derived;
`persisted::rebuild_derived`, which restore runs, puts back empty ones and rebuilds the index, so a
restored world is never poisoned. A broken boundary invariant (an outbox event, an RNG stream, an
open invocation, a spawn or despawn recorded in the index but not in the world) is `sim.internal`
and poisons the world, at the start of `step` (system `sim.begin`) or in `sim.finish`; it trips in
release builds too.

## 10. "No staged structural change" is checked through the index

`bevy_ecs` 0.19.1 keeps the world's command queue private, so the constant-time check confirms
instead that every spawn and despawn the index recorded since its last compaction happened in the
world: each spawned entity exists and carries its `EntityId`, each despawned one is gone. It costs
the tick's spawns and despawns, not its entities.

## 11. Invocations and boundary writes as transactions

`begin_invocation`, `commit_invocation` and `rollback_invocation` take the marks of simulation.md
4.5 (allocator, RNG table, outbox, event counter, decision requests); a rollback also removes
entities spawned through the immediate API, so a host that applies as it goes is covered as well as
one that stages. `Boundary::mark` and `rollback` undo a refused write's ids, spawns and events
(simulation.md 4.2); a write validates before it changes components or despawns, so nothing else
needs undoing.

Spawns go through `entity::spawn` (immediate), the system parameter `SimCommands` (deferred to the
system's return), or `entity::reserve` and `entity::spawn_allocated`: a script's `spawn` reserves
its id at call time and the host spawns the entity under it before it commits (script-host.md 5.4).
Only a reserved, unspawned id is accepted, and a reservation ends when its invocation commits or
rolls back, when the tick ends, or, for one made at a boundary, when the next tick begins; anything
else is `sim.entity_id_invalid`, so a committed id never comes back (simulation.md 7.2), even after
`sim.finish` has compacted the index. Events go through `event::emit` (the outbox during a tick, the
inbox at a boundary) or the system parameter `Emit`.

## 12. Sorted iteration helpers

clippy reports `Query::iter` and `iter_mut` even when followed by `sort_unstable`, so
`pocket_sim::order::by_id` and `by_id_mut` wrap the sorted form of simulation.md 8.2 with the lint
allowed in one place; a query using them must read `&EntityId`. Choice 4's wrapper system parameter
is not built. `order::shuffle_storage` is the storage-shuffle hook: it clones every entity in an
order drawn from its own generator, then despawns the originals (despawning each right after its
clone would swap the clone back into the original's row), and rebuilds the index; components must be
`Clone` to survive it.

## 13. `EntityId::from_f64` has no `next`

It refuses with `sim.entity_id_invalid {id}` (as text, since the value may be NaN): without the
world there is no allocator to name.

## 14. The component registry

A `ComponentRegistry` resource per world (class Ignored: its `ComponentId`s belong to that world)
holds each component's name, origin, version, doc, `ComponentId`, Rust `TypeId` for engine types,
JSON Schema (schemars for engine types, built from the fields for project ones) and, for components
scripts name, script-host.md 7.1's `ComponentSchema`, whose data types (`FieldType`, `FieldValue`,
`FieldSchema`, `ProjectValues`) are declared here. `ComponentSchema::new` checks a declaration whole
before anything registers: names, a doc per field, no field twice, enum variants present and
distinct, defaults of their field's type, finite (numeric.md 7) and in range (a tick within 2^53 -
1, an enum index below the variant count), and at most 65,535 numeric slots, since slots are `u16`
(`sim.component_field_invalid {component, field}`, the reason in the message). The slot accessors
(`ComponentAccess`) and the dynamic registration of project components stay in `pocket-script`,
which then calls `ComponentRegistry::register_project`.

## 15. Decoding checks what constructors check

Persisted types whose constructors refuse values (`Tick` past 2^53 - 1, `TickRate` outside 1 to
1000, `WorldSeed`, `EntityAllocator` and `EventCounter` outside their ranges, `Name`, `EventKind`,
and `EventInbox`, whose boundary mark must not pass its events, whose sequence numbers must ascend
and whose data must be canonical) have a hand-written `Deserialize` that decodes their derived shape
under the same serde name and then calls the constructor, so a snapshot or replay cannot bring in a
rate of 0 or a mark that panics later (persistence.md 3.6), while their encoding and JSON Schemas
are unchanged. Tracing them needs samples: `persisted::record_samples` records one for each type
that refuses the tracer's 0 or empty string, and `persisted::tracer_config` turns on samples for
structs, which `EntityAllocator` and `EventCounter` are.

## 16. The physics rows as built

Simulation.md 4.4 rows 4 to 9 and 8.5, as `pocket-physics` builds them. The force systems read the
state at the tick's start and evaluate the sea and the wind at `(n - 1) / rate` for tick n, computed
from the tick as `time()` is, not as `time() - dt()`. `physics.step` removes, in id order, the
bodies whose entity is gone or lost its `Collider` or `Transform`, or whose `RigidBody` or
`Collider` changed (seen through a hash kept in the body's `user_data`, not through change
detection), then builds the new ones in id order, so Rapier's arena reuses its slots the same way in
a world and its fork. A changed body is rebuilt: removed and made again in the same step.

`physics.contacts` emits `physics.contact_began` and `physics.contact_ended` with the lower id as
subject and `{other}` as data, the events of one pair in the order the step reported them. A body
removed while touching (despawned, or its `Collider` taken away) ends each of its contacts, and a
rebuilt one ends them and begins them again in the same tick, the end first: `physics.step` keeps
the removed colliders' entities for the step, since Rapier reports those ends (with its `REMOVED`
flag) for colliders no longer in its set. Anything that tracks touching pairs from the events sees
every pair that began end.

No physics component requires another (`bevy_ecs`'s `#[require]`): a restore inserts components
section by section, and a required component would come back at its default on an entity of the
original world that had none, so the fork would differ from its source (persistence.md, P2). A
`Collider` without a `Transform` gets no body and is `sim.body_invalid` (`reason` "no Transform")
each tick; a body without a `Velocity` or an `ExternalForce` moves as if both were zero, and shows
neither. The bundles of `pocket_physics::sailing` insert every component a body uses.
