# Actions and the sailing intents: slice 2 decisions

Status: Draft, slice 2

The decisions slice 2's implementation of actions (the module `action` of `pocket-interface`, the
act requests of `pocket-runtime`'s `play` module) took where
[actions.md](../../shared/contract/actions.md) and [sailing.md](../../shared/contract/sailing.md)
(Actions, Codes, Checks) were ambiguous or wrong for the code. Those files are the shared contract,
hashed in `shared/SYNC.toml` (charter 6.2; checks.md 5.4: a changed file fails
`gen.shared_modified`), so this line records its choices here, numbered, until the contract takes or
replaces them, as [perception-slice2.md](perception-slice2.md) does for perception. The time model's
are in [time-slice2.md](time-slice2.md).

## 1. World state as built

`IntentInstance` is the contract's with the id as a plain `u64` and one field added, `actor`: the
entity id of the seat's body when the intent started, which the executor perceives through and whose
controls it sets; an instance whose seat no longer has that body fails with `internal.error` the
next tick. Its `failure` is stored as `{code, detail}` (`StoredProblem`) and its message rendered
from the game's template for the code, or the contract's, whenever it is shown. Its `progress` is a
list of `(name, PlainData)`, each value stored rounded at the precision the intent declares
(`ProgressDef` carries a `unit`, as a `FactDef` does, so a bearing is stored in [0, 360)), so the
world hash covers what an agent can see and nothing finer.

A control is latched in the field the simulation reads, not in a copy: the sailing controls bind to
`Boat`'s target fields (`rudder`, `sheet`, `hoist`), which the physics reads, and `Controls` keeps
only unbound latched values and waiting pulses (`ControlBinding::{Unbound, Engine, Field}`). One
value per control is then world state, and a control set directly and one an executor wrote are the
same write. A pulse bound to a project component's field (`interact` to the sample's `Crew.take`) is
delivered by `interface.intents` at the start of the tick it acts in, by writing its target's id
into that field through a writer the runtime installs (`FieldWriter`); the game's rule reads and
clears it.

## 2. Executors read an actor view

Executors, affordance requirements and validation read the world only through the `ActorView` trait
(`action::view`): instruments, the entities the seat knows (seen, remembered or charted, with its
facts) and its perceived events. Perception's `PerceptionView` implements it (`perception::bridge`),
so actions see exactly what the seat perceives; tests use a `BoatView` over the sailing instruments
with a listed set of known entities. The catalog (`ActionCatalog`, class Ignored) names the view
factory and how the seats' bodies are found (the entities whose `Observer` names a seat).

## 3. Affordances come from the perception declarations

A kind's affordances are carried by perception's `KindDef.affordances` (the sailing ones in
`samples/sailing/perception.json`) and read by the action layer as `AffordanceDef`s, with `effect` a
nested object as the contract's listing writes it. `ActionCatalog::validate` checks the declarations
at load: names unique, channels named, every effect an intent that takes the kind as its target or a
pulse that targets it, and every `Fact` requirement on an instrument or on a fact a player may know
(`Hidden` and `Owner` facts of another entity are `definition.invalid`).

## 4. The act request in the runtime

`act` is a Request (threads.md 5.5) whose session part (an answer to the seat's pending decision
with `resume`, the events delta from the seat's push cursor, a lockstep act queued for its boundary)
stays outside the world, and whose world part is the Write it produces, `act.apply`: the call
validated whole and applied at the boundary, recorded with the canonical call (seat named, defaults
filled, references as ids). A replay applies `act.apply` through the live path, so validation and
every intent's `accept` run again. Who acted is the command's `Source` (a player by its seat's
index, a developer, the editor, the host), which stands for `ActOrigin`; an act a script makes
inside a tick (`ctx.intent`) is applied as a developer's, never recorded. `act.apply` is host-only:
`act` applies it through the game's own path under the act's source (`Game::apply_produced`, as a
replay applies it), and sent directly by anyone else it is refused with `command.host_only`, since
it would skip the session's part: the lockstep queue, `resume`, the push cursor and the seat's idle
time, so idle decisions would be raised for a seat that acted.

A refused act leaves no write: the recorder keeps a `Refused` note (replay.md's), which no replay
applies, and the world, the intent counter and the seat's events are as before (the
`contract.actions.atomic` test compares the hash over every persisted section).

`intents` and `affordances` answers carry `warnings` when the request used a declared alias.

A call that fails while applying (an engine bug, `internal.error`) is undone whole by the action
layer itself, not only by the runtime's boundary rollback (which undoes spawns and events): the
intent table, the body's `Controls`, its engine-bound control values, the event inbox and counter
and the turn state are taken before applying and put back.

## 5. The interface in every game

Every game the runtime builds installs the sailing showcase's interface after the engine's
components are registered: perception with the declarations of `samples/sailing/perception.json`
(compiled in, so a replay and the web build have them), the sailing action catalog, the continuous
time rules, the script host's bridge (`pocket_script::interface::InterfaceBridge`,
script-host-slice2.md), the `FieldWriter` and a stepped session. A world with no observer naming a
seat has it inert. A game's own interface declarations wait for the project definition that declares
them (shared/contract/README.md, The game definition).

The runtime's `FieldWriter` writes a pulse's target into the bound field of a project component
through the field's schema, as `world_edit` writes it, inserting the component with its defaults on
a body that lacks it; a game that declares no such component has no rule to read the pulse, which
then acts on nothing, as an unbound pulse no rule reads, rather than faulting the tick. An entity
field takes the target's id live or not, so a crate destroyed at the boundary its pulse was queued
at reaches the rule, which emits `interact.ignored` with `sail.crate_gone`, instead of faulting the
tick on a dead reference. So `use Crate1 take_aboard` in the sample sets `Crew.take`, the sample's
`take_aboard` rule takes the crate in that tick and emits `crate.taken`, and the recording replays
tick for tick (`pocket-runtime`'s `tests/play.rs`). The sample's scene does not seat the skipper
yet: scenes name only the engine's slice 1 components and project components, not perception's
`Observer` and `Perceivable`, so the runtime's tests seat it in code (the scene format's extension
is the scene's owner's).

## 6. Checks as built

- **`contract.actions.refusal`** varies every field of `act`, its actions and the sailing intents'
  parameters, `intents` and `affordances` without proptest (no new crate): up to 64 single-edit
  variants per field in a fixed order (camelCase, the unit suffix left out, letters dropped,
  doubled, swapped, changed). A variant that is the contract's declared alias (a unit suffix left
  out: `heading` for `heading_deg`) is accepted with `request.alias_used`; every other is refused
  with `request.unknown_field` whose suggestions name the field, and nothing is applied (3,082
  refused, 8 aliases).
- **`contract.intents.transparency`** leaves out `EventCounter` and `EventInbox` with `IntentTable`
  and `ObserverEvents`: the intents' own events take sequence numbers and sit in the inbox, so the
  other events are compared by kind, tick, subject and data. The check runs 3,600 ticks in the test
  suite and its 18,000 under `--release --include-ignored`.
- **`contract.actions.atomic`** puts each of six valid actions before each of twelve invalid ones
  (four of phase 1, five of phase 2, three of phase 3).

## 7. The helm holds a course with an integral term

The slice 1 boat has a strong weather helm: it needs about 0.45 of rudder to hold a close-hauled
course, and rounds up with the rudder centred. sailing.md's PD law
(`HELM_KP * error / 45 - HELM_KD * yaw_rate / 30`) alone would sail with a steady error, so the helm
adds an integral term (`HELM_KI = 0.8` on the error's integral in degree-seconds over 45, bounded at
0.6 of rudder, accumulated only within 20 degrees of the heading so a long turn does not wind it
up), as an autopilot's rudder offset does. Weather helm mirrors with the tack, so the integral
changes sign when the true wind comes over the other side. `HELM_KP = 1.6`, `HELM_KD = 1.2`.
Measured: a turn of 180 degrees from a reach is reached in 582 ticks at 6 m/s, the slowest of the 42
turns of the acceptance matrix.

## 8. `sail_to` beats at its best angle and allows for leeway

The sailing simulation's `CLOSE_HAULED_DEG` (50) is the angle the boat can point, not the one it
sails best at: at 50 degrees off the wind it stalls and falls off, and its velocity made good to
windward peaks between 60 and 65 degrees (0.58 to 0.62 m/s at 6 m/s; `tests/explore.rs`,
`explore_polar`). `sail_to` beats at `BEAT_DEG = 60`. Close-hauled the boat makes about 14 degrees
of leeway, so a straight leg would slide downwind of its target: `sail_to` keeps a smoothed leeway
estimate (heading to course over the ground, a 5 s time constant, held below 0.5 m/s) and steers the
heading that puts the course over the ground on the target. It goes straight for the target when its
bearing is at least `BEAT_DEG` plus the leeway off the wind (the layline, with 5 degrees of
hysteresis), and on a beat it tacks at the corridor's edge or when its tack's course no longer
closes on the target (near the target the corridor is wider than the distance). `vmg_mps` is
measured along the course over the ground.

## 9. Sailing acceptance as measured

At 6 m/s from the west: `come_to_heading` succeeds between every pair of the seven headings 45
degrees apart outside the no-go zone (270 is in it) within 1,800 ticks at five seeds, identical per
seed; `trim_sail {sheet: "best"}` reaches `drive >= 0.9` on both tacks of every point of sail; a
mark 150 m dead upwind is reached in 29,700 ticks with 9 tacks, and one 500 m upwind in 96,275 ticks
with 17 tacks (0.3 m/s made good: 25 to 30 s lost a tack), so the 500 m limit is 144,000 ticks (40
minutes) and `sail_to`'s 900 s default timeout is too short for it (the tests give 3,600 s). The
five seeds give the same results: the scene's breeze is steady and nothing in it draws on the seed.
The 500 m beat and the full matrix run under `--release --include-ignored`; the default suite runs
four turns and the 150 m beat. The local check's test step runs the whole workspace that way
(`cargo test --workspace --release -- --include-ignored`, outside `--quick`), the ignored sailing
and transparency checks included.

The acceptance worlds are the game's: the skipper acts through its perception (the sailing
declarations and `game_catalog`, `tests/common/play.rs`), not the test-only `BoatView`, so an entity
a test lays is known once a tick's perception update has seen it (the tests step once after laying a
mark or a crate). Measured on them: the same 582 ticks for the slowest turn, the 150 m beat in
29,700 ticks with 9 tacks, the 500 m beat in 96,273 ticks with 17 tacks at each of the five seeds
(two ticks fewer than the `BoatView` run, which acted a tick earlier).

The costs, measured (`tests/timing.rs`, release, on R1 while other agents' builds shared it;
reported, never a pass condition, and not yet profiled): `act.call`, a call of four actions
validated and applied in the sailing scene, 1.52 ms at the median; `intent.tick`,
`interface.intents` with `trim_sail` and `sail_to` live, 66 µs.

## 10. The web test

`action::web` carries out the three sailing intents with physics and perception for 3,600 ticks and
reports each intent's end and the hash of every persisted section at every tick;
`examples/web_actions.rs` is the same as a WebAssembly module with no imports, which
`crates/pocket-interface/tests/web/run.mjs` runs under Node against the native report (the
`web_actions` test writes it to `<target>/tmp/pocket-interface-web-actions.txt`). In a browser, the
same page and worker as perception's web test (`tests/web/index.html` and `worker.js`, with the
module's name changed to `web_actions.wasm` and the native report as `expected.txt`) load through
`tools/webcheck.py --serve`. The local check runs the browser form in its web step
(`xtask/src/steps/web_interface.rs`): the native report from the `web_actions` test in release, the
module built for wasm32 in release, the page staged in `out/check/web-actions/` and loaded in a Web
Worker of headless Chrome, whose report must equal the native one byte for byte; Node is not a tool
the check needs. Measured: `sail_to` arrives at tick 2643, the trim and the heading are holding at
tick 3600, and the release WebAssembly build's 3,600 hashes are identical to the native debug
build's under Node (442 ms) and in headless Chrome (569 ms in a Web Worker).

## 11. A control's value is latched as recorded

The canonical record writes a control's value as the wire does (`pocket_contract::codes::num`),
where an integral number is a JSON integer, so `-0.0` is recorded as `0`. A `set` therefore latches
`x + 0.0` (`refs::wire_value`), which turns `-0.0` into `+0.0` and leaves every other value as it
is: the value applied live and the one a replay applies from the record carry the same bits
(persistence.md hashes an `f64` by its bits). An intent's parameters and a point target are recorded
as given (`serde_json` writes `-0.0` there), so they need nothing. `tests/actions.rs`,
`a_recorded_set_of_negative_zero_replays_to_the_same_world`.

## 12. Player indices are the declared seats'

`Source::Player(index)` is a player's identity, so an index names a seat the catalog declares, by
its place among the declared seats, and nothing else: `SeatRow.index` is `None` for a seat an
observer names that the game does not declare. Such a seat is still a row, so a developer may act,
list and step for it and the time controller keeps its decisions, but no player plays it, and a body
that goes never moves another seat's index (the first build numbered undeclared seats after the
declared ones in `EntityId` order, so an earlier one's body going handed its index to the next).
Refusing an undeclared observer outright (`definition.invalid`) was the alternative; it would have
refused tests and tools that seat a body for a developer to drive. `tests/player_view.rs`,
`player_indices_name_declared_seats_only`.

## 13. A seat's controls are those its body takes

`ControlBinding::Engine` and `Field` carry `writable(world, body)`: the sailing controls need a
`Boat` on the body, `interact` takes any body (the game's field writer gives a body the component it
lacks). Validation treats a control the body cannot take as not the seat's
(`action.unknown_control`, its suggestions and `allowed` the controls the body takes), and an intent
whose channels hold such a control as not the seat's (`action.unknown_intent`), both in phase 2; a
`use` whose effect the body cannot carry out is refused the same way in phase 3. An executor's write
the body can no longer take (it lost its `Boat` since the intent started) fails the intent with
`internal.error` and makes none of that tick's writes, never faulting the tick; a pulse whose body
can no longer take it acts on nothing. `tests/actions.rs`,
`a_body_without_a_boat_takes_no_sailing_control`.

## 14. Targets are named through the seat's view

`intent_view` names an intent's target as its seat knows it (`ActorView::known`, perception's view
for the game), never from the world's `Name`: a target the seat has forgotten or never knew is
answered as `{id}` whether it still exists or not, so `intents` cannot tell a player whether an
entity it no longer perceives is still there. The script host's records use the same function.
`tests/player_view.rs`, `intents_answer_a_forgotten_target_alike_whether_it_exists_or_not`.

## 15. The budgets of `intents` and `affordances`

Both answers keep the longest prefix of their list (intents newest first, affordances ranked as
perception ranks percepts) whose compact JSON, with the count of what was left out and the 16 bytes
reserved for `tokens`, fits `budget_tokens * 4` bytes, and report `tokens`, the estimate of the
answer without that member (perception.md, Token budgets). `intents` always carries `omitted` (the
contract's answer lists it); `affordances` carries it only when something was left out, as the
contract's example shows none. The default budget is the seat's observer profile's (400 for the
sailing skipper), the maximum 16,000 (`request.out_of_range` above), and a budget that cannot hold
the empty list is refused with `perception.budget_too_small` and the smallest that would.
`tests/actions.rs`, `intents_and_affordances_answer_within_their_budget`.
