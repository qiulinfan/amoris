# Time: slice 2 decisions

Status: Draft, slice 2

The decisions slice 2's implementation of the time model (the module `time` of `pocket-interface`,
the time requests of `pocket-runtime`'s `play` module) took where
[time.md](../../shared/contract/time.md) and [sailing.md](../../shared/contract/sailing.md) (Time)
were ambiguous or wrong for the code. The contract is hashed in `shared/SYNC.toml`, so they are
recorded here, as [actions-slice2.md](actions-slice2.md) records the actions'.

## 1. Two pacings: the session's and the loop's

The contract's `Pacing` is the session's (`PlayPacing`: stepped, lockstep, real time with
pause-on-decision and a thinking clock), kept by the session's time controller (`Controller`) beside
the slice 1 loop model (`TimeModel`, `Pacing::{Stepped, RealTime}`), which still times the game
thread's and the worker's ticks. `Controller::pace` gives the loop's answer for every pacing (the
halt, the episode's end and a turn's deciding phase stop ticks in all of them; lockstep runs a tick
when every seat has committed through it; real time holds while a decision with time left or a pause
source holds, waking for the next thinking clock to run out).

Slice 2 drives the controller from the synchronous `Game` (`time::session`): `step`, `commit`,
`continue`, `act` with `resume`, and `time.status`. On the game thread a `step` that is time.md's
request (a player's, or one with any member beside slice 1's `ticks`: `until`, `seat`, `observe`,
...) runs through the session one tick per boundary (`time::stepping`, driven by the loop through
`pocket_runtime::play::LoopStep`), so the queue is served between its ticks as time.md, Threads and
the web, asks: the loop runs each tick as it runs every tick (published, its events streamed), and
after it the run computes the decision points and stops where a session's `step` stops, in the same
order (`session::Progress` is shared by both). While it runs, another `step` is refused with
`time.busy`, as is a time.md `step` while slice 1 steps are queued. A developer's `step {ticks}`
keeps slice 1's form there (its answer lists the failed invocations of its ticks), and the session's
decision points are computed after those ticks too, so none is lost between requests. `act`,
`continue`, `pause`, `resume` and `time.status` reach the game through `Game::apply` as elsewhere.
The web worker still decodes slice 1's `step {ticks}` (pocket-web is outside this work); real time
with pause-on-decision on the game thread waits for the loop to take the controller's `pace` (left
for the MCP and real-time play work). The controller's logic is tested with its clock injected, as
the loop injects its own; `pocket-runtime`'s `tests/thread_play.rs` steps a player to each decision
on the game thread, serves a developer's `status` while a step runs and refuses a second step
meanwhile.

## 2. Requests as built

- `StepRequest` also takes `seat`: the seat whose decisions and perception `until` follows (a
  player's own; a developer names one, or follows the only seat). Its `ticks` defaults to 1 and 0
  answers at once, so slice 1's `step {ticks}` is the same request; the runtime's answer adds the
  world hash after the last tick, which the slice 1 checks read.
- A `step` or `commit` in a turn's deciding phase answers `stopped: "waiting"`: no tick can run
  until the seats in `to_move` end their turns.
- `observe` in a time request is perception's `ObserveRequest`, answered in its projection with the
  seat's intents and pending decision (`action::observe::seat_parts`).
- The events delta of an answer is the seat's perceived events after its push cursor, kept per seat
  in the controller, in the JSON projection within `budget_tokens` (default 400).
- `DecisionReason::Requested` names the declared reason `name`: the variant's tag is already
  `reason`, so the listing's `Requested {reason, event}` would write the key twice.

## 3. Thinking clocks

A running clock is kept as the instant it started and the time it had; it runs out at that instant
plus the time, compared with the loop's clock as it is, so a wait until it lands on it rather than a
rounding short of it (a loop could otherwise wait 10^-13 ms at a time). The episode's `start`
decision arises before any wall time is known; its clock starts at the first real-time boundary.

The resume bound of time.md, Checks (2.3 ms after the clock runs out, on the reference laptop)
assumes the loop's 1 ms waits; on this machine (Windows, no raised timer resolution) a 1 ms sleep
takes up to about 16 ms, and the test measured 21.7 ms, so it asserts 50 ms and prints the measure.
The game loop's own wait (threads.md 3.2) decides the bound once it takes the controller.

## 4. Lockstep in one process

`commit` extends the seat's commitment and runs the world as far as every seat has committed,
answering `stopped: "waiting"` with the seats it waits for when it cannot reach the caller's;
`until` ends the commitment at the tick it holds. An `act` of a seat committed beyond the world's
next tick is validated at once, answered `pending: true` with its `applied_at`, queued, and applied
at the boundary before its seat's next uncommitted tick (in seat order, then submission order),
validated again there: one that no longer validates is dropped and its seat gets `action.dropped`
with its next answer. Peers, their hash exchange and `time.desync` wait for networked play.

## 5. Decision points

A developer's session (`Controller::all_requests`) keeps requested decisions a player would not
receive (their event unseen and their declaration not `from_instruments`); a player's drops them, as
time.md says. Decision points are computed only for ticks a session runs; a tick run outside a
session (`Game::run`, a replay, the checks) raises none, which changes nothing in the world.

## 6. Wall time in the synchronous game

The runtime's session reads wall time from a `WallClock` resource the host installs (the game
thread's clock natively, which `GameThread::spawn` installs, `performance.now()` in a worker);
without one it reads 0, so a run stops at its ticks, `until`, the episode's end or a halt, never at
a wall limit. Nothing in a tick reads it. A `step` the game thread's loop drives reads the loop's
clock directly.

## 7. Episodes

`TimeRules` (class Ignored) carries the game's structure and its `done` and `quiet` predicates;
`interface.turns` latches the episode's end in the `Episode` resource (world state) the tick `done`
holds. The sailing sample declares no `done` yet: its course rule (a fixture's,
shared/benchmark/tasks.md 6.1) comes with the benchmark's tasks.

## 8. Checks as built

`contract.time.pacing_equivalence` runs both halves in-process with the loop's clock injected: real
time at speed 4 with pause-on-decision and an agent stand-in answering after a seeded delay of up to
1.5 s and starting seeded intents, 7,200 ticks; and two seats in lockstep committing seeded chunks
of 1 to 300 ticks and acting at seeded boundaries (their acts queued), 7,200 ticks. Each replayed in
stepped pacing from its applied acts gives the same hash over every persisted section at every tick.
The wall-limit check uses a clock that advances 1 ms a reading instead of a slowed scene.

## 9. A developer's omniscient `until`

`step {omniscient: true}` from a developer (a checker does not step) evaluates `until` through
`time::omni`: perception's raw omniscient view measured from the seat's body (every entity, every
fact, hidden ones included) and the world's events numbered by their `EventSeq`, with the seat's
instruments read as the seat reads them; the request-time checks of `until` are skipped, and the
answer is marked. The first build accepted the flag but still read the seat's perception, so a
condition on what the seat could not know never held. A player's `omniscient` is refused with
`perception.omniscient_forbidden` before any view is built (`tests/time.rs`).

## 10. Pauses and halts

`pause` and `resume` are `time::pause`'s, answered with the caller's `time.status`:

- `pause` takes real-time pacing only (`time.wrong_mode` otherwise, naming the requests the pacing
  takes): a developer's pause, or the caller's seat's when the session allows the seat to pause
  (`Controller::may_pause`, session policy, empty by default; `time.cannot_pause` otherwise). A
  developer's pause now holds real time in `Controller::pace` too, not only through the loop's
  `TimeModel`.
- `resume` by a developer ends its pause and, in any pacing, a halt; a seat's ends only its own
  pause, in real time. A hot update that lands ends a halt as well (`Controller::hot_update_landed`,
  called by the runtime's `scripts.swap`). Before this the runtime had no way to end a halt: a
  script failure stopped every later `step` for good.
- The runtime's game offers both as Controls; being stepped, it refuses `pause` with
  `time.wrong_mode`.

`tests/pacing.rs` holds real time with a developer's pause and an allowed seat's, refuses a seat not
allowed, halts on a `script.*` error until a developer resumes (a seat's resume does not end it),
and in stepped pacing answers a player's `step` during a halt with `stopped: "halted"` and the
generic `time.halted`.

## 11. Not built in slice 2

- `wait`, `time.set_pacing` and `time.visibility`: the runtime's sessions are stepped, and real time
  and lockstep exist in the controller, driven by tests with the loop's clock injected; `wait` comes
  with the game thread's loop taking the controller (section 1).
- `commit` is the controller's (`time::session::commit`) and not a runtime command, for the same
  reason.
- The lockstep writes check (time.md, Checks): refusing with `time.wrong_mode` an editor's write or
  a hot update sent outside the lockstep input, broadcasting them as lockstep inputs, and peers'
  hash exchange with `time.desync`, which wait for networked play.

## 12. `until` on an owner's fact

A fact declared with `owner` exposure is shown to its owner only (perception's `shows`: `owner`
reaches a percept only in the omniscient view), so `until` refuses a condition on such a fact of
another entity with `perception.not_perceivable`, as it refuses a `hidden` one; on the seat's own
body it is the seat's to wait on (its instruments are its `owner` facts). The first build refused
only `hidden` facts, so a condition on another boat's `owner` fact was accepted and the run went to
its tick limit for nothing. `tests/time.rs`, `until_is_refused_on_what_the_seat_cannot_know`.
