# Sailing

- Status: Draft, slice 0. Maintained in Amoris's `shared/contract` (README, Contract record).
- Charter: 2.4.1 (the sailing showcase), 3.1, 3.4, 3.5, 7 (items 8, 9, 10 and 12).
- Contract version: 0.2 (draft).

## What this fixes

The sailing showcase's declarations for the contract, in one place: the skipper's perception (its
profile, instruments, kinds, events and an example observation), its controls and intents with their
completion and failure, crates taken aboard, its time structure and pacing, and its error codes.
They are the sailing game's, built on the general rules of [perception.md](perception.md),
[projection.md](projection.md), [actions.md](actions.md), [time.md](time.md) and
[errors.md](errors.md). The numbers in them are design choices for slice 2, not measurements, and
the player benchmark tunes them. The sailing game itself (buoyancy, the sail's force, the keel) is
simulation, specified with slice 1; this file names what it must expose (`best_sheet`, the no-go
angle, `afloat`) and nothing of how it computes it.

## The skipper's perception

The showcase's player perceives the sea from the boat it sails (charter 2.4.1: "wind direction and
strength, the boat's heading, speed and sail state, and nearby islands and marks"). The game has one
seat, `skipper`, whose body is the boat it sails; one seat per body (README, Seats and callers).

Profile `skipper`: `sight {range_m: 2000, fov_deg: 360, eye_m: (0, 2, 0), occlusion: true}`,
`hearing {scale: 1}`, `attention_m: 300`, `memory_s: 600`, `memory_capacity: 64`,
`event_capacity: 256`, `sightings: ["mark", "boat", "crate"]`, `chart: true`, `positions: true`,
`budget_tokens: 400`.

Instruments, in this order:

| Instrument | Unit | Precision | Meaning |
|---|---|---|---|
| `heading_deg` | bearing | 0 | Where the bow points. |
| `course_deg` | bearing | 0 | Direction of travel over the ground. |
| `speed_mps` | m/s | 1 | Speed through the water, positive ahead. |
| `wind_from_deg` | bearing | 0 | The true wind's direction: where it blows from. |
| `wind_mps` | m/s | 1 | The true wind's speed at the boat, gusts included. |
| `twa_deg` | angle | 0 | True wind angle off the bow, positive when the wind comes over the starboard side. |
| `awa_deg` | angle | 0 | Apparent wind angle off the bow, same sign. |
| `aws_mps` | m/s | 1 | Apparent wind speed. |
| `point_of_sail` | enum | | By `abs(twa_deg)`: `in_irons` below 45, `close_hauled` 45 to 60, `close_reach` 60 to 80, `beam_reach` 80 to 100, `broad_reach` 100 to 150, `running` from 150. |
| `tack` | enum | | `starboard` when the wind comes over the starboard side (`twa_deg > 0`), else `port`. |
| `hoist` | fraction 0..1 | 2 | How much sail is set. |
| `sheet` | fraction 0..1 | 2 | 0 hard in, 1 eased right out. |
| `trim` | enum | | `furled`, `luffing` (eased too far), `good`, `overtrimmed` (in too far), from the sailing simulation's angle of attack. |
| `drive` | fraction 0..1 | 2 | The sail's drive as a share of the best drive at this apparent wind. |
| `rudder` | fraction -1..1 | 2 | Positive turns the bow to starboard. |
| `heel_deg` | angle | 0 | Positive heeled to starboard. |
| `pos_m` | position | 1 | Where the boat is. |
| `afloat` | bool | | The hull is in the water and not aground. |
| `next_mark` | text | | The course's next mark, or `none`. |
| `taken`, `left` | count | 0 | Crates aboard and still adrift. |

Kinds:

| Kind | `detect_m` | `height_m` | `priority` | Chart position | Facts (exposure) |
|---|---|---|---|---|---|
| `island` | 3000 | its terrain's height | 50 | its centre, at load | `radius_m` (chart, 0), `shore_m` (coarse, 0, relative: distance from the observer's boat to the island's nearest shore), `shore_brg_deg` (coarse, relative: bearing of that shore point) |
| `mark` | 400 | 2 | 80 | where it is laid, at load or by the task's setup | `color` (chart), `round_to` (chart: `port` or `starboard`), `order` (chart), `next` (chart, bool) |
| `boat` | 1500 | 6 | 70 | none | `heading_deg` (coarse), `sail` (coarse: `set` or `furled`), `speed_mps` (full, 1) |
| `crate` | 120 | 0.5 | 40 | none | `alongside` (full, bool, relative: within reach of the observer's boat to be taken aboard) |

`shore_m`, `shore_brg_deg` and `alongside` depend on the observer, so their derived functions take
the observer's body and they are declared `relative: true`: memory does not store them, and a
remembered island's are recomputed from its remembered position when it is shown (perception.md,
Perceivable entities). Islands carry `Occluder`: a mark behind an island is not seen until the boat
opens it past the headland, and a boat sailing behind one drops into memory. Every island and every
course mark is on the chart, at its chart position, as on a real chart, so `sail_to` can name a mark
before it is in sight; a mark a rule moves after it is charted shows at its chart position until it
is seen (perception.md, The perception update).

Events: `crate.taken` (sight; data `taken`, `left`), `mark.rounded` (private to the rounding seat;
data `mark`, `order`), `course.finished` (private; data `marks_rounded`), `boat.aground` (sight;
data `island`), `sail.set` and `sail.furled` (sight), `interact.ignored` (private; data `code`,
`message`: a pulse the game rule could not act on, Taking a crate aboard), `sighted` (from the
perception update), and the intent lifecycle events (private; actions.md, The lifecycle). Every
other kind is `Hidden` (perception.md, Events and their scope).

An observation at the default budget, the call `observe {"since": 40}`, with the boat running east
toward the first mark, an island to the north-east, a crate it passed now out of sight astern, and
another islet only on the chart:

```text
tick 1800 t=30.0s seat=skipper observer=Sloop#12
self heading_deg=087 course_deg=089 speed_mps=3.4 wind_from_deg=270 wind_mps=6.1 twa_deg=-177 awa_deg=-176 aws_mps=2.7 point_of_sail=running tack=port hoist=1.00 sheet=0.95 trim=good drive=0.91 rudder=0.02 heel_deg=1 pos_m=(12.4,0.0,-3.1) afloat=true next_mark=Mark1 taken=3 left=13
intent #3 sail_to active target=Mark1#40 distance_m=412 bearing_deg=092 vmg_mps=3.4 eta_s=121 leg=direct tacks=0
see Mark1#40 mark brg 092 rng 412m color=yellow round_to=port order=1 next=true
see Isle#7 island brg 041 rng 380m radius_m=150 shore_m=231 shore_brg_deg=044
mem Crate4#21 crate brg 268 rng 135m age=4.5s
chart Islet2#9 island brg 175 rng 1210m radius_m=60
event 41 tick=1795 crate.taken Sloop#12 brg 000 rng 0.0m taken=3 left=13
```

It is 774 bytes, 194 tokens by the estimate (counted with Python's `len(text.encode())` over the
block); the text projection reserves nothing for a `tokens` member (projection.md, Text projection).
The event's subject is the observer's own boat, so its horizontal range from the boat's origin is 0
and its bearing is written 000 (perception.md, Geometry). The merged candidate sequence is Mark1,
event 41, Isle, Crate4, Islet2; at `budget_tokens: 185` the longest prefix that fits with its
omitted line is the first three (739 bytes), so the last two percepts give way to
`omitted crate=1 island=1 (nearby {"kinds":["crate","island"]})`. In the JSON projection the same
mark begins `{"id":40,"name":"Mark1","kind":"mark","visibility":"seen","detail":"full"` and has
`"bearing_deg":92`, `"range_m":412` and `"pos_m":{"x":424.2,"y":0.0,"z":11.3}` (an `EntityId` shown
as a number is only an illustration: its wire form is spec-sim's).

## Actions

The seat `skipper` (charter 2.4.1). The controls and intents are the sailing game's declarations;
the physics they drive (the rudder's and sail's forces, buoyancy, the keel) is the sailing
simulation's, specified with slice 1, which MUST provide what is named here: the actuator rates,
`best_sheet(awa_deg)`, the no-go half-angle, the close-hauled angle, and the `afloat`, `aground` and
`trim` readings.

### Controls

| Control | Kind | Range | Default | Channel | Meaning |
|---|---|---|---|---|---|
| `rudder` | axis | -1 to 1 | 0 | `helm` | The rudder's target; positive turns the bow to starboard. The blade follows at `RUDDER_RATE` per second. |
| `sheet` | axis | 0 to 1 | 1 | `sail` | The sheet's target: 0 hard in (boom on the centreline), 1 eased right out. The winch follows at `SHEET_RATE`. |
| `hoist` | axis | 0 to 1 | 0 | `sail` | How much sail to set; 0 furled. Follows at `HOIST_RATE`. |
| `interact` | pulse, targets `crate` | | | `hands` | Take the targeted crate aboard; validated against `take_aboard`'s requirements (Taking a crate aboard). |

Recommended for the sailing simulation: `best_sheet(awa) = clamp((abs(awa) / 2 - 5) / 80, 0, 1)`,
which puts the boom at about half the apparent wind angle, the usual trim rule for a sail at a
working angle of attack; the simulation's own model decides, and `trim` reports `luffing` or
`overtrimmed` against it.

### `come_to_heading`

Turn the boat onto a heading and, with `keep`, hold it there (an autopilot). Channels: `helm`.
Target: none.

| Parameter | Type | Default | Meaning |
|---|---|---|---|
| `heading_deg` | number, 0 to under 360 | required | The heading to come to. |
| `tolerance_deg` | number, 0.5 to 45 | 5 | How close counts as on the heading. |
| `turn` | `shortest`, `port`, `starboard` | `shortest` | Which way to turn. A turn the other way than the shortest may take the bow through the wind (a tack) or away from it (a gybe). |
| `settle_s` | number, 0 to 30 | 2 | How long the heading must stay within tolerance. |
| `keep` | bool | false | Hold the heading once reached. |
| `timeout_s` | number, 1 to 600 | 60 | Until it must be reached. |

- **Succeeds** (or starts holding) when `abs(error) <= tolerance_deg` for `settle_s` without a
  break, where `error` is the signed angle from the heading to the target. The executor keeps the
  forced turn direction in its state until the remaining error is within 90 degrees, then steers by
  the shortest error.
- **Fails** with `sail.aground` the tick the boat goes aground; with `sail.lost_steerage` when the
  speed through the water stays below `MIN_STEERAGE_MPS` (0.3) with the error outside tolerance for
  `STEERAGE_S` (10 s), which is what happens to a boat left head to wind (in irons); with
  `intent.timeout`.
- **Warns** at acceptance `sail.no_go_zone` when the target heading is within `NO_GO_HALF_DEG` (45,
  as master's `Boat` had none of the sail's drive within 45 degrees of the wind's eye: master
  `docs/design/physics.md`, Boats) of the wind's direction: the boat can point there but will stop.
  It warns `sail.not_set` when the sail is furled and the boat has too little way to steer.
- **Progress**: `heading_error_deg` (angle, 0), `turning` (`port`, `starboard` or `steady`),
  `on_heading_s` (seconds within tolerance so far, 1).
- **Steering**: `rudder = clamp(HELM_KP * error / 45 - HELM_KD * yaw_rate_dps / 30, -1, 1)`, with
  the gains the sailing simulation's tuning gives; `yaw_rate_dps` is derived from `heading_deg`
  between ticks and kept in the executor's state.

### `trim_sail`

Set the sail and trim the sheet, to a value or to the best trim for the wind, and with `keep` go on
trimming as the wind changes. Channels: `sail`. Target: none.

| Parameter | Type | Default | Meaning |
|---|---|---|---|
| `sheet` | number 0 to 1, or `"best"` | `"best"` | The sheet to trim to; `best` follows `best_sheet(awa_deg)`. |
| `hoist` | number 0 to 1 | unchanged | How much sail to set first. |
| `tolerance` | number 0.005 to 0.2 | 0.02 | How close the sheet must come. |
| `keep` | bool | false | Keep trimming once reached (with `best`, auto-trim). |
| `timeout_s` | number, 1 to 120 | 20 | Until it must be reached. |

- **Refused** at acceptance with `sail.not_set` when the sail is furled (`hoist` instrument 0) and
  `hoist` is not given; the detail suggests `{"hoist": 1}`.
- **Succeeds** (or holds) when the `sheet` instrument is within `tolerance` of the target and the
  hoist within 0.02 of its target.
- **Fails** with `intent.timeout`. While holding it does not fail on a gust: the target moves and
  the winch follows.
- **Progress**: `sheet`, `target_sheet` (fractions, 2), `trim` (enum), `drive` (fraction, 2).

### `sail_to`

Sail to a point or an entity: straight there when the wind allows, beating to windward in tacks when
it does not, trimming the sail on the way unless told not to. Channels: `helm`, and `sail` when
`trim` is `auto`. Target: an entity of kind `mark`, `boat` or `crate` that the seat knows (seen,
remembered or charted), or a point `{x, z}` (an island is not a target: its centre is on land; sail
to a point off its shore instead).

| Parameter | Type | Default | Meaning |
|---|---|---|---|
| `arrive_m` | number, 1 to 500 | 10 | Within this distance of the target counts as arrived. |
| `trim` | `auto`, `manual` | `auto` | `auto` also sets and trims the sail (it occupies `sail`); `manual` leaves the sail to the caller. |
| `timeout_s` | number, 10 to 3600 | 900 | Until it must arrive. |

- **Refused** at acceptance with `sail.target_on_land` when the target point lies within the chart
  radius (`radius_m`) of an island the seat knows; with `perception.unknown_entity` for a target the
  seat does not know.
- **Warns** `sail.land_on_course` when the straight line from the boat to the target passes within
  `radius_m` of a known island (naming it and the distance along the line), and `sail.not_set` when
  `trim` is `manual` and the sail is furled. The executor does not plan a route round land
  (actions.md, open choice 3): the agent splits the passage into legs.
- **Succeeds** when the distance from the boat to the target is at most `arrive_m`. A target entity
  that moves is followed at its perceived position; a remembered one at its remembered position; a
  charted one at its chart position.
- **Fails** with `sail.aground`; with `sail.no_progress` when the distance has not fallen by
  `NO_PROGRESS_M` (5) within `NO_PROGRESS_S` (60 s); with `perception.target_lost` when a target
  entity is no longer known to the seat (forgotten, or seen to be gone); with `intent.timeout`.
- **Progress**: `distance_m` (0), `bearing_deg` (bearing, 0), `vmg_mps` (velocity made good toward
  the target, 1), `eta_s` (distance over `vmg_mps` when that is above 0.1, else absent; 0), `leg`
  (`direct`, `port_tack`, `starboard_tack`), `tacks` (count).
- **Steering**: when the bearing to the target is at least `CLOSE_HAULED_DEG` (from the sailing
  simulation, about 50) off the wind's direction, the executor steers for it directly. Otherwise it
  beats: it sails close-hauled on one tack, starting with the tack whose close-hauled heading is
  nearer the bearing to the target, and tacks when the other tack's close-hauled heading would carry
  it to the target (the layline) or when its distance from the line through the target along the
  wind exceeds `max(4 * arrive_m, 0.25 * distance_m)`, which keeps it in a corridor rather than
  sailing to the edge of the sea. Each heading change is made as `come_to_heading` steers (the same
  gains); with `trim: auto` the sheet follows `best_sheet` and the sail is hoisted if furled.

### Taking a crate aboard

Kind `crate` offers `take_aboard`: `effect: pulse interact`, requires `Seen` and `Within {max_m: 3}`
(master's island took a crate aboard within 3 units: master `samples/island/scripts/main.tsx`,
`REACH`). Because `interact` is the effect of that affordance, a raw
`{"do": "pulse", "control": "interact", "target": "Crate7"}` is validated against the same
requirements and refused with `action.unavailable` when they are not met (actions.md, Affordances),
so an agent never gets a success answer for a pulse that would do nothing. The game rule that reads
the `interact` pulse moves the crate aboard and emits `crate.taken`; when it cannot (another boat
took the crate at the same boundary), it emits the private event `interact.ignored` with the code
`sail.crate_gone` and its message, so the agent learns why. In master the crate came aboard by
itself when the boat passed close; in the rebuild it is an action, so the agent decides, and an
agent that only steers alongside gets nothing.

A typical sequence, each call one action request or one step (time.md):

```text
act   start trim_sail {hoist: 1, sheet: "best", keep: true}  -> intent #1, holding in seconds
act   start sail_to, target Mark1                            -> intent #2; #1 superseded
step  {ticks: 36000, until: "decision"}                      -> stopped: decision (#2 succeeded)
act   use Crate4 take_aboard                                 -> an interact pulse; crate.taken
```

The second line shows the channel rule at work: `sail_to` with `trim: auto` occupies `sail`, so it
supersedes the holding `trim_sail`, and the sail stays trimmed because `sail_to` trims it. With
`trim: manual`, `sail_to` occupies only `helm` and the two run together.

## Time

- **Structure**: continuous.
- **Pacing**: stepped for the tests and the player benchmark (spec-mcp), the default; real time with
  `pause_on_decision: true` for a watched match and for a human and an agent sailing the same match
  (charter 10, slice 4), with a thinking clock whose initial time, increment and maximum the player
  benchmark sets from the agents' measured decision times; no agent has played yet, so slice 0
  measured none.
- **Decision filter** for the skipper seat by default: events `intent.succeeded`, `intent.failed`,
  `intent.reached`, `sighted`, `boat.aground`, `mark.rounded`, `course.finished`, `crate.taken`;
  `idle_s: 10`; `every_s: 120`. An agent that has set `sail_to` toward a mark is not asked again
  until it arrives, fails, sights something new or two minutes pass. The game declares no requested
  decisions (time.md, Decision points).
- **Episode**: the game's `done` predicate holds once the course rule has set `Course.finished_tick`
  (a fixture's, shared/benchmark/tasks.md 6.1), and the outcome's `result` readings are `finished`
  (bool), `marks_rounded`, `crates_taken` (counts) and `aground` (bool) (time.md, Episodes).
- A typical agent loop in stepped pacing is three calls a decision: `act` (start an intent),
  `step {ticks: 36000, until: "decision", observe: {}}` (which returns the next decision with an
  observation), and the agent's own reasoning. At 60 ticks a second a ten-minute leg is one `step`.
- **As built on Pioneer (0.2)**, in `samples/sailing-course` (`docs/spec/player.md` 8): the loop
  is `observe`, `act`, `wait` (a `step` until the next decision point, time.md); the default
  filter adds `interact.ignored`, so a crate the crew could not take aboard is a decision too; the
  episode ends in the tick the course rule emits `course.finished` (rounding a mark is coming
  within 15 m of it, its side not judged), and the outcome's `result` is that event's data,
  `marks_rounded`; `finished`, `crates_taken` and `aground` are not reported yet. The reference
  skipper finishes the course in 12 decisions and 5148 ticks (`docs/bench/player.md`).

## Codes

The sailing game's codes (errors.md, Game codes):

| Code | Use | When | Detail | Message template |
|---|---|---|---|---|
| `sail.no_go_zone` | warn | `come_to_heading` toward within `NO_GO_HALF_DEG` of the wind. | `heading_deg`, `wind_from_deg`, `off_wind_deg`, `no_go_half_deg` | `Heading {heading_deg} is {off_wind_deg} degrees off the wind (from {wind_from_deg}); within {no_go_half_deg} the sail cannot drive and the boat will stop.` |
| `sail.not_set` | refuse (`trim_sail`), warn | The sail is furled where it is needed. | `hoist`, `suggest` (`{"hoist": 1}`) | `The sail is furled; hoist it first, for example with "hoist": 1.` |
| `sail.target_on_land` | refuse | `sail_to` a point within a known island's chart radius. | `island`, `radius_m`, `distance_m` | `That point is on {island}, {distance_m} m from its centre within its {radius_m} m radius.` |
| `sail.land_on_course` | warn | The straight course passes within a known island's radius. | `island`, `along_m` | `{island} lies on the direct course {along_m} m ahead; sail_to does not steer round land.` |
| `sail.aground` | fail | The boat touched land. | `island`, `pos_m` | `The boat ran aground on {island}.` |
| `sail.lost_steerage` | fail | Too little way to steer, off the heading, for `STEERAGE_S`. | `speed_mps`, `heading_error_deg`, `for_s` | `The boat lost steerage: {speed_mps} m/s, {heading_error_deg} degrees off for {for_s} s; bear away from the wind to gather way.` |
| `sail.no_progress` | fail | `sail_to` came no nearer by `NO_PROGRESS_M` in `NO_PROGRESS_S`. | `distance_m`, `needed_m`, `window_s` | `No progress toward the target: still {distance_m} m away after {window_s} s.` |
| `sail.crate_gone` | warn | An `interact` pulse found its crate gone at the boundary it applied (carried by `interact.ignored`). | `crate` | `{crate} was no longer there to take aboard.` |

## Checks

- **Sailing acceptance** (slice 2, in the local check command with the showcase): `come_to_heading`
  between every pair of the 8 headings 45 degrees apart whose target is outside the no-go zone, wind
  6 m/s, succeeds within a tick limit slice 2 sets from the sailing simulation (none exists in slice
  0, so nothing was measured); `trim_sail {sheet: "best"}` reaches `drive >= 0.9` on each point of
  sail; `sail_to` a mark 500 m dead upwind succeeds with at least one tack within a tick limit set
  the same way; sailing at an island ends `sail.aground`; a 1-second timeout ends `intent.timeout`;
  an `interact` pulse at a crate 14 m away is refused with `action.unavailable`. Each at 5 seeds,
  results identical per seed.
- **Golden cases**: a `sail_to` ending in `sail.no_progress` shows its code, message and detail
  through `intents` and its code and message in the `intent.failed` event line (mcp.md 5;
  actions.md, The lifecycle).
