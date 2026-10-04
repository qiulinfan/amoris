# Threads: slice 1 decisions

Status: Draft, slice 1

The decisions slice 1's implementation (the crates `pocket-runtime` and `pocket-link`,
`pocket-interface`'s time model and `pocket-web`'s worker) took where [threads.md](threads.md) was
ambiguous or wrong for the code. They are open choices 8 to 15 of threads.md 13, which lists them by
title and keeps their numbers here. They moved here because threads.md would otherwise have passed
the 800-line limit that charter 3.9 set before version 0.4, as simulation.md's did.

## 8. The game thread as built

`GameThread::spawn` takes `ThreadOptions {stack_bytes, clock, on_publish, pacing}`. The time model
is `pocket_interface::TimeModel`: `Stepped` and `RealTime {speed}` with pauses (lockstep, turns and
decisions come in slice 2). The loop's Controls are `step {ticks}`, answered after its last tick
with `{tick, world_hash, errors}` (the ticks' failed invocations, at most 20) and refused while the
world is poisoned, and `time_control {pause?, pacing?}`; `status` and `snapshot` are Reads.
`TimeStatus` is time.md's subset `{tick, t_s, pacing, paused, halted, behind_ms}`.

## 9. One world, no workers, no wire

A `Game` holds main only (no branches, 3.6, until slice 2's sessions). A Request (`scripts.apply`)
runs its stages inline on the game thread at its boundary, as the headless form does (5.5), so a
compile holds that boundary until a worker pool is built. `ReplyValue::Fork` and the wire module
(7.2) are not built.

## 10. Publication, sources, shutdown

Version 1 is published when the game is built; later snapshots and stream events only once
`GameHandle::reader` has been called (4.3). A world poisoned mid-tick is not at a boundary, so the
last publication stays the state before that tick and `status` answers `poisoned: tick` without a
hash. `client(Source::Host)` is `source.in_use`. Shutdown bypasses the queue's capacity
(`push_unbounded`) and answers what is queued or held with `game.stopped`; a panic outside
`Sim::step` stops the thread with `game.stopped` naming it, kept by `SnapshotReader::stop_reason`.

## 11. `world_edit` as built (slice 1 review)

The check simulates the call in order, so every edit is checked against the world as the edits
before it leave it and nothing is applied unless all pass (charter 3.4; 5.4): an edit naming an
entity an earlier edit of the call destroys, by reference or through an entity field, is
`sim.entity_not_found {id, path, destroyed_by}`, a second `destroy` of one entity included (refused
rather than a no-op, since for that edit the entity no longer exists); a second `set` of a component
starts from the first one's result, and a `set` after a `remove` from the component's defaults. An
edit cannot name an entity an earlier edit of the same call spawns, whose id is not known until it
is applied: it is refused like any unknown name, and a second call names it. Once the check passes,
applying cannot fail but through an engine bug (`sim.internal`, the edits before it staying
applied). A game's own Write (`GameBuilder::command`) must likewise validate before it changes a
component or despawns, since a refused Write is undone only in its spawns, ids and events
(`Boundary::rollback`).

## 12. Sequences, faults and restores (slice 1 review)

A source's next seq lives on the handle, so a client handed out again after its predecessor dropped
continues the source's sequence (5.1) and an envelope the old client left held never shares a seq
with the new one's. A `step` of a poisoned world is refused with `sim.world_poisoned` and records
nothing, so one fault is one `Fault` record; on the game thread a poisoned world pauses the time
model (real time stops asking for ticks; a resume asks once, is refused and pauses again) and
republishes the last published world with `halted` and `paused` set, since a poisoned world has no
snapshot of its own. `Game::restore` while recording ends the open segment (unless a fault closed
it) and opens the next with `Rebase {cause: Restore}` (replay.md 2.5); a refused restore rebases
onto the world it left as it was, so the recording goes on either way. `time_control`'s and
`ThreadOptions`' real-time speed must be finite, above 0 and at most `MAX_SPEED`, 1,000 (chosen, not
measured: a 60 Hz game asks for a tick every 17 microseconds there), else
`request.out_of_range {path: "/pacing/real_time/speed", got}` and nothing changes; `pocket run`
refuses such a `--realtime`, and a `--seconds` that is not finite and at least 0, with exit code 2
before the game starts.

## 13. The worker as built (slice 1, the web task)

`pocket-web` runs the game role in a module Web Worker (`web/game.js`) over `WorkerCore`
(`crates/pocket-web/src/worker.rs`), the loop of 7.3 written once for every target so native tests
drive it with their own clock. A task drains the commands that arrived since the last one as the
batch of its first boundary, sorted by `pocket_link::canonical_order` (5.2; the commands are
`Envelope`s whose `ReplyTo` is none, since each is answered by a `reply` message keyed by its source
and seq), holds those whose `at` is later and answers `command.tick_passed` to those whose `at`
passed; it then runs boundaries and ticks until the pacing says wait or 8 ms are spent, and answers
`now` (the script re-posts through a `MessageChannel`), `at` (a `setTimeout`) or `command`. The
Controls are the game thread's (8): `step {ticks}`, answered after its last tick with
`{tick, world_hash, errors}`, `time_control {pause?, pacing?}`, and `snapshot`, answered with its
tick, writes and hash. The queue's capacity counts queued and held commands (`queue.full`); `close`
answers everything waiting with `game.stopped`. `status {state, tick, since_ms}` is posted when the
loop starts or stops running ticks on its own (`ticking`, `waiting`, `halted`), not at every
boundary, since in real time the loop waits between every two ticks.

Slice 1 (review): the page holds no `GameClient`, so the worker enforces what a client guarantees
natively (5.1). A `cmd` from `"host"` is `source.in_use {source: "host"}`, as `client(Source::Host)`
is (10): Host is the runtime's own producer, and `Game::apply` exempts it from `command.host_only`,
so a page sending as Host could apply `scripts.swap`. A `cmd` whose `seq` is not above the last one
queued from its source is `request.out_of_range {path: "/seq", got, min_exclusive}` and is not
queued, so a repeated or earlier seq can neither jump the canonical order nor leave a promise of the
page unsettled; the review asked for `request.invalid_value`, but the error protocol keeps that code
for a name outside an enumeration and `request.out_of_range` for a number outside its range. As
`GameClient::send` does, a seq is taken only when its command is queued, so a `queue.full` refusal
can be sent again with the same seq.

Slice 1 (review): a poisoned world halts the web game and never stops it (8; 12). It has no snapshot
of its own (`persist.not_at_boundary`), so wherever the loop would publish (after a poisoning tick,
at an `ack` that lets a skipped publication go, after a Write that succeeds on the poisoned world)
it posts the last snapshot it posted again, a new version with the same bytes and hash and
`time.halted` set, carrying the hashes and events gathered since; the hash stream thus reaches the
last good tick even when flow control skipped the publications before the fault, and the page's
latest snapshot is the last one posted, which may be older than that tick (natively it is the tick
before the fault, since the thread publishes every tick). Its steps are answered with the fault, the
pacing pauses and `status {state: "halted", error}` is posted; commands are answered as before
(`step` with `sim.world_poisoned`). A resume of an already poisoned world runs no tick and counts
none: it is refused, halts and pauses again. The event stream takes the inbox's events whose
`EventSeq` is past the last one streamed, rather than an index reset by every step, since a tick
that poisons never replaces the inbox (the outbox becomes the inbox only in `sim.finish`) and an
index reset would send the last tick's events again.

The pacing is a copy of `pocket_interface::TimeModel`'s slice 1 subset
(`crates/pocket-web/src/pace.rs`, with the same constants), because architecture.md 5 gives
`pocket-web` no edge to `pocket-interface` and `pocket-runtime` re-exports the time model only under
its native feature `thread`. The copy is a bounded stopgap: it goes when `pocket-runtime` exports
the time model (or a loop core both the game thread and the worker drive) on every target.

## 14. The web messages as built (slice 1, the web task)

7.2's messages with these departures, each because `pocket-link`'s wire module (9) is not built:

- `init`'s `project` is the **web package**: one JSON file (`pocket_web::package`, format
  `pocket-web-package` version 1) holding the manifest's name, rate and seed, the scene and the
  scripts compiled natively as `CompiledSet` JSON, transferred as one buffer. The shipped module has
  no transpiler (architecture.md 8.3), and `pocket pack <project> --web` (architecture.md 4.13) is
  not built, so `pocket-web`'s native example `pack` writes it
  (`cargo run -p pocket-web --example pack --release -- <project> <out.json>`), compiling through
  `pocket-runtime`'s `transpile` as a dev-dependency, which never reaches the wasm build. It goes
  when `pocket pack --web` exists. A package whose scripts are not what the native build compiled
  shows as a different world hash in the cross-target comparison.
- `snap` carries the **whole snapshot's canonical bytes** (`Snapshot::to_bytes`, every section,
  caches included, as if `init` always asked `with_caches`) in one transferred buffer, instead of
  the changed sections and `kept`. The page rebuilds every snapshot with `Snapshot::from_bytes` and
  compares its hash with the worker's `world_hash` (11, "Web ordering and rebuild").
- `snap` carries **`hashes`**: every tick's world hash since the previous `snap` posted, as
  `[tick, hash]` pairs, tick 0 in the first. A skipped publication skips a snapshot, never a tick's
  hash, so the page has the whole `TickHash` chain in order whatever its flow control did; that is
  how the page reports per-tick hashes and how the web check compares them with the native chain.
  The worker therefore takes the world hash after every tick, from the snapshot it posts or alone.
- `events` are JSON records `{seq, id, event}` (the stream number, the `EventSeq` and spec-sim's
  event as JSON), since the page has no decoder of the canonical encoding outside the module;
  `missed` is always 0 (nothing drops events between the worker and its page).
- `registry` is `{components, formats}` as JSON, posted with the first `snap` and whenever the
  bundle changes.
- `cmd`'s `params` is a JSON text, as 7.2 says; the envelope is decoded strictly (unknown fields
  refused with suggestions) and its `source` by `source_from_json`.
- The worker also posts `started {instantiate_ms, build_ms, time_origin}` before `ready`, for the
  start-up measurements; and the check page alone sends `perf` (the kept tick timings) and `verify`
  (a replay replayed in the worker, `pocket-check`'s `verify_bytes`), answered `perf` and
  `verified`.

## 15. The page's handle and what was measured (slice 1, the web task)

`web/pocket.js` is 7.4's handle (`start(module, packageBytes, {seed, pacing})`, `ready`,
`command(name, params, {source, at})`, `latest()`, `onEvents`, `close()`) with `onTicks` (the hash
stream), `onSnapshot`, `onStatus`, `view()` (the named bodies of the latest snapshot, through
`SnapshotView`) and the check's `perf()` and `verify(bytes)`. Each source's seq counts from 1 in the
order the page sends. The page acknowledges a `snap` once rebuilt, not drawn; the check also runs a
page that acknowledges 25 ms late, so the worker's flow control is exercised. `web/form.js` is the
browser-local run form: real time, a top-down view of the latest snapshot on a 2D canvas (rendering
proper is slice 3), and a helm whose `world_edit`s go as the local player.

Measured on R1 (headless Chrome 154, the sailing sample, `web` profile, isolated loads, 2026-10-03,
three loads while other agents' builds ran): the module after `wasm-bindgen` is 8,235,660 bytes raw
and 2,608,557 gzipped at level 9, of which 1,259,928 are the name section; after `wasm-opt -O3`,
which drops it, 6,530,080 and 2,415,874. Rust is 5.65 MB of the 6.58 MB of code (Rapier and parry
the largest part), QuickJS-ng and wasi-libc 0.91 MB. A tick in the worker took 0.205 to 0.375 ms at
the median (p95 0.38 to 1.38 ms, 1,200 ticks a load) and its world hash 0.10 to 0.17 ms more, where
the native `pocket hashes` takes about 0.30 ms a tick with its hash (3,000 ticks, minus a run of 0);
a publication took 0.14 to 0.26 ms and a snapshot's rebuild on the page 0.145 to 0.19 ms. Start-up
from navigation on a local server: the module compiled on the page in 69 to 112 ms (its fetch
included) and was ready 690 to 820 ms after navigation, building the game in the worker took 184 to
216 ms, and the first snapshot reached the page 870 to 1,140 ms after navigation; one load under
heavy machine load took 10.6 s. `cargo xtask check`'s web step writes the current numbers as
measurements.
