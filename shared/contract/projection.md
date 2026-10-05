# Projections

- Status: Draft, slice 0. Maintained in Amoris's `shared/contract` (README, Contract record).
- Charter: 3.1 (the rendered screen, a text projection for LLMs and a tensor projection for RL are
  projections of one perception layer), 3.6, 7 (item 8).
- Contract version: 0.1 (draft).

## What this fixes

How perception's answers ([perception.md](perception.md), Queries) and the answers that carry
perceived events (`act`, [actions.md](actions.md); the time requests, [time.md](time.md)) are
written: the rounding every projection shares, the JSON projection, the text projection an LLM reads
(shared byte for byte across runtime targets, README, Open choices 2), the tensor projection for RL
observers, and the image. It was split from perception.md, which defines what is perceived.

## Rounding

A fact or instrument value is projected at its declared precision: the value is formatted with
Rust's `format!("{:.*}", precision, value)`, which rounds the exact binary value to the nearest
decimal and ties to even; a result of `-0` (any number of zeros) is written `0`; a `Bearing` that
rounds to 360 is written 0. In JSON the value is the number that string denotes, written as an
integer when the precision is 0. Amoris gets these bytes from one Rust formatter across runtime targets. Checked with rustc 1.98.1 on this machine, printing `format!("{:.*}", p, v)` for each
pair from a program built with `rustc -O`: `0.5`, `1.5`, `2.5` at precision 0 give `0`, `2`, `2`;
`0.25` and `0.35` at 1 give `0.2` and `0.3` (0.35 is stored just below a tie); `-0.04` at 1 gives
`-0.0`, which the rule writes `0`; `359.6` at 0 gives `360`, which a bearing writes `0`.

`range_m` of percepts and events has precision 0 when the range is at least 100 m and 1 below it,
and `bearing_deg` likewise in JSON; in the text projection a bearing is always three integer digits
(`BBB`, below), whatever the range. `age_s` has precision 1; `pos_m` has the precision of the
profile's `positions` (1). Both measures are horizontal and from the body's origin (perception.md,
Geometry).

**Stored values.** Where world state keeps a value already rounded, the stored value is
`format!("{:.*}", p, v)` parsed back with `str::parse::<f64>`, with `-0` stored as `0` and a bearing
of 360 as 0, so all runtime targets store the same bits. Which stored fields are rounded:

| Stored field | Rounded |
|---|---|
| `MemoryEntry.facts` (perception.md, Observers) | Yes, each at its fact's precision: remembered values are what the observer saw |
| `MemoryEntry.pos_m` | No: the exact position, rounded only when projected |
| `PerceivedEvent.bearing_deg` and `range_m` in the ring | Yes, by the range rule above, fixed at the event's tick |
| `PerceivedEvent.data` | Yes, each at its field's precision |
| `IntentInstance.progress` (actions.md, Intent instances) | Yes, each at its reading's precision |

Ranking compares rounded values (perception.md, Token budgets). Rounding is otherwise applied to the
projection only.

## JSON projection

The types under perception.md's Queries, serialized with `serde_json` in field declaration order,
compact (no spaces). Optional fields that are `None` are omitted, except `omniscient`, which is
always present.

## Text projection

The text an LLM reads, shared byte for byte between the lines (README, Open choices 2). One item a
line, LF line ends, ASCII except inside names and text facts, which are UTF-8. Its grammar:

```text
observation := header instruments intent* decision? entity* event* omitted?
nearby      := header-short entity* omitted?
describe    := header-short entity affordance* event*
events      := header-short event* omitted?
events-delta:= event* omitted-events?         (in the text block of act and time answers)
header      := "tick " TICK " t=" SECONDS "s seat=" SEAT " observer=" REF [" OMNISCIENT"] NL
header-short:= "tick " TICK [" OMNISCIENT"] NL
instruments := "self" (" " NAME "=" VALUE)* NL
intent      := "intent #" ID " " INTENT " " STATUS [" target=" (REF | POINT)] [" tag=" VALUE]
               (" " NAME "=" VALUE)* NL
decision    := "decision " DECISION_ID (" " REASON)* NL
REASON      := "event:" SEQ ":" KIND | "idle:" S | "interval:" S | "turn:" N | "start"
               | "requested:" NAME
entity      := VIS " " REF " " KIND " brg " BBB " rng " RANGE "m" [" age=" SECONDS "s"]
               (" " NAME "=" VALUE)* [" can=" VERB ("," VERB)*] NL
affordance  := ("can " VERB | "cannot " VERB ":" (" " CODE " " MESSAGE)+) NL
event       := "event " SEQ " tick=" TICK " " KIND [" " REF] [" brg " BBB " rng " RANGE "m"]
               (" " NAME "=" VALUE)* [" cause=" SEQ] NL
omitted     := "omitted" (" " KIND "=" COUNT)* [" events=" COUNT] [" lost=" COUNT]
               " (" HINT ("; " HINT)* ")" NL
omitted-events := "omitted" " events=" COUNT [" lost=" COUNT] " (" HINT ")" NL
REF         := [NAME] "#" ENTITY_ID
POINT       := "(" X "," Z ")"
VIS         := "see" | "mem" | "chart"
BBB         := a bearing rounded to an integer and written as three digits, zero-padded ("007", "270")
VALUE       := number | "true" | "false" | enum value | text | "(" X "," Y "," Z ")"
```

- `SECONDS` in the header has one decimal; `S` in a reason is the declared seconds at their own
  precision; `TICK`, `N` and counts are integers. `DECISION_ID` is the decision point's id (time.md,
  Decision points).
- A text value matching `[A-Za-z0-9_.:+-]+` is written bare; any other is written as a JSON string
  (quoted, escaped). A `MESSAGE` is always written as a JSON string.
- Facts with a `Bearing` unit are written as three digits like `BBB`; other numbers at their
  precision with fixed decimals (`3.40` at precision 2), so the column width of a value does not
  jump between observations.
- A section with nothing to show writes nothing; `self` is always written in an observation.
- **The omniscient view** writes `seat=- observer=-` in the header (it has no observer) and ends the
  header line with ` OMNISCIENT`. The header is the first line of the contract's projection; the MCP
  layer may put its own `world` line before it (mcp.md 4.1), so the mark is on the header line, not
  on the first line.
- **`describe`** of an entity writes its `entity` line, then one `can VERB` or `cannot VERB:` line
  per verb its kind offers, a `cannot` line listing every unmet requirement's code and message, then
  the last five perceived events whose subject it is, as `event` lines.
- **Act and time answers** (`act`, `step`, `commit`, `wait`, `continue`) in the text projection
  carry their events delta as `event` lines in their text block, after the answer's own JSON line,
  never as JSON `PerceivedEvent`s; an observation they carry follows as its own block (mcp.md 4.3).
- **Tokens.** The text projection has no `tokens` member, so nothing is reserved for one: an
  answer's text must fit `budget_tokens * 4` bytes. The JSON projection reserves 16 bytes for its
  `tokens` member (perception.md, Token budgets).

The text of an observation is `header` and the rest in the grammar's order; no line is ever wrapped.
A worked example for the sailing skipper is in [sailing.md](sailing.md), The skipper's perception.

## Tensor projection

For RL observers the profile's `TensorSpec` fixes shapes once, so every observation of that profile
has the same arrays (Gymnasium's `Dict` of `Box` spaces; `describe` lists each block's shape, low
and high).

```rust
pub struct TensorSpec { pub blocks: Vec<TensorBlock> }

#[serde(tag = "block")]
pub enum TensorBlock {
    /// f32[n]: the named instruments, encoded as below.
    Instruments { name: String, instruments: Vec<String> },
    /// f32[count, 4 + features]: the `count` highest-ranked percepts of these kinds; per row:
    /// present (1 or 0), sin and cos of the relative bearing, range / sight.range_m, then each
    /// named fact encoded as below. Rows past the last percept are zero.
    Nearest { name: String, kinds: Vec<String>, count: u32, facts: Vec<String> },
    /// f32[count, kinds.len() + 1]: `count` rays spread evenly over `fov_deg` about the heading,
    /// each giving a one-hot of the first perceivable kind it meets (or none) and the hit
    /// distance / range_m (1 when nothing is hit). Unity ML-Agents' ray perception sensor.
    Rays { name: String, count: u32, fov_deg: f64, range_m: f64, kinds: Vec<String> },
}
```

Encoding: numbers as `f32` of the unrounded value (an RL policy has no token cost to save;
determinism holds because the value is deterministic); booleans 0 or 1; enumerations one-hot over
their declared values; bearings and angles as their sine and cosine from the deterministic math
library (spec-sim), relative to the heading. Facts a percept does not show at its visibility are 0,
with the `present` column telling the policy which rows are real. The tensor answer is
`{tick, omniscient, tensors: {name: {shape, data}}}`, `data` row-major; a transport MAY carry `data`
as base64 of little-endian `f32` (spec-mcp). `budget_tokens` does not apply to tensors and is
refused with `request.not_applicable`.

## The image (humans)

The rendered image is the human seat's projection (charter 3.1, 4.4; rendering is slice 3's). Its
HUD shows the human seat's instruments from the same `InstrumentDef`s. A game with fog of war draws
an entity of a fogged kind only when the human seat's observer has it `seen` or `remembered`.
Rendering never feeds perception: occlusion here comes from colliders, headless, never from depth.

## Checks

- **`contract.projection.golden`**: shared cases in `shared/contract/conformance/projection.jsonl`,
  each an answer in JSON (an observation, a `nearby`, a `describe` of an entity, an `events` answer,
  an `act` answer with an events delta) and its expected text; all runtime targets render each to the
  expected bytes. The cases cover rounding ties, negative zero, bearings near 360 and at zero range,
  a bearing below 100 m (three digits in text, one decimal in JSON), quoting of text values, every
  section empty and full, each decision reason, an intent with a target entity, a target point and a
  tag, a `cannot` line with two unmet requirements, the omniscient header
  (`seat=- observer=- ... OMNISCIENT`), and an observation of a seat bound to `omniscient_player`
  (perception.md, The omniscient view).
- **Stored rounding**: a fact value whose rounding differs between `(x * 10^p).round() / 10^p` and
  the parsed formatted decimal is stored as the latter in memory, in the ring and in an intent's
  progress, on all runtime targets.
