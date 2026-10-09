# Errors

- Status: Draft, slice 0. Maintained in Amoris's `shared/contract` (README, Contract record).
- Charter: 3.4, 3.6, 4.2.6, 6.1, 7 (item 12).
- Contract version: 0.2 (draft).

## What this fixes

An illegal action returns a structured error `{code, message, detail}` with the reason, so the agent
can correct itself; unknown parameters and fields are refused with a "did you mean ..." suggestion,
and nothing in the call is applied; the engine never accepts them silently (charter 3.4). This file
fixes the error object, where it appears, the codes and their details, how suggestions are computed,
how several problems in one call are reported, and the messages.

Master learned this the hard way. An agent passed `world.set` its fields under the wrong key, was
answered `{"ok": true}`, nothing changed, and it spent a dozen calls reading the engine's C++ to
find out why (master `docs/mcp.md`, Asking the engine how to call it; `docs/agent-eval.md`, Watching
the agents changed the interface). After commands refused unknown parameters and named the ones they
took, and fields were refused with the nearest name
(`Light has no field 'colr'; did you mean 'color'?`), failed calls in a run of fifty-seven tasks
fell from 48 to 15 (master `docs/agent-eval.md`, Fifty-seven on opencode). Amoris's predecessor charter
lists the same lesson and the "did you mean" mechanism among what it carries over.

Precedents: RFC 9457 (problem details for HTTP APIs: a type, a human-readable detail, extension
members) for an error object with a stable machine code and a free-form detail; gRPC's status codes
for the distinction between a malformed argument, a failed precondition and a missing thing;
Stripe's error object, whose `param` names the offending field, for `path`; JSON Pointer (RFC 6901)
for paths; Kubernetes' strict field validation, which rejects unknown fields instead of dropping
them; rustc's "a field with a similar name exists" suggestions and its edit-distance threshold for
"did you mean".

## The problem object

```rust
/// One problem: a refusal, a warning (now or later), or an intent's failure.
pub struct Problem {
    /// Dotted snake_case, `<family>.<reason>`: "request.unknown_field".
    pub code: String,
    /// One or two English sentences for the agent, built from the code's template and the detail.
    pub message: String,
    /// Always an object (possibly empty); its fields are fixed per code (the tables below).
    pub detail: serde_json::Map<String, serde_json::Value>,
}
```

This is the wire form. A problem kept in world state (an intent's failure) is stored as its code and
its detail as plain data, and its message is rendered from the template whenever it is shown, so no
English text enters a world hash (README, World state and wire forms).

On the wire:

```json
{"code": "request.unknown_field",
 "message": "come_to_heading has no parameter 'headng'; did you mean 'heading_deg'?",
 "detail": {"path": "/actions/0/params/headng", "field": "headng", "suggestions": ["heading_deg"],
            "allowed": ["heading_deg", "tolerance_deg", "turn", "settle_s", "keep", "timeout_s"]}}
```

- `code` is the contract: an agent and a checker branch on it. Codes are stable within a major
  version (README, Versioning).
- `message` is for reading. It is deterministic (the same problem gives the same bytes on both
  lines) but a program never parses it.
- `detail` carries the facts as data: the JSON Pointer `path` of the offending part of the request
  when there is one, the value given (`got`), what was expected, `suggestions`, `allowed`. Its
  fields per code are part of the contract; adding one is a minor change.

The engine builds problems through one constructor per code (a `codes` module in the contract types,
README, Open choices 1), which fills `detail` and renders `message` from the code's template, so no
call site writes a message of its own.

## Where problems appear

| Use | Meaning | Carried in |
|---|---|---|
| Refusal | The whole call is refused and nothing of it is applied. | The call's answer is the problem (one, with the rest in `detail.also`; Several problems). |
| Warning | The call succeeded, with something worth knowing. | `warnings: [Problem]` in the success answer. |
| Failure | An intent ended unsuccessfully (actions.md). | The intent's `failure`, whole, in the `intents` answer (actions.md, Intent instances; mcp.md 5); its code and message in the `intent.failed` event's data. |
| Later warning | Something went wrong after the call was answered (a lockstep action dropped, a thinking clock ran out). | The session's `warnings` in the caller's next answer; session state, never a world event. |
| Halt | The simulation halted after a script error (spec-script's `script.*` problems). | `halted` in time answers (time.md, Halts): the script's problem for developers and checkers, `time.halted` for players. |

A refusal is the only use the charter's "nothing in the call is applied" governs. A warning never
stands in for a refusal: a field the engine does not take is refused, not accepted with a warning.

The transports carry the problem whole, never reworded or flattened (their envelopes are spec-mcp's
and spec-script's):

- **MCP**: a refused tool call is a tool result with `isError: true` (the MCP specification reports
  tool errors in the result so the model sees them), the problem in `structuredContent` and its
  `message` followed by the compact JSON in the text content.
- **JSON-RPC**, where a transport uses its error responses: `error.message` is the problem's
  message, `error.data` the problem, `error.code` -32602 (invalid params) for the `request.*` family
  and -32000 for the rest.
- **Scripts**: a host API call a script makes with a bad argument throws an error whose `code`,
  `message` and `detail` are the problem's (spec-script).

## Codes

A code is `<family>.<reason>`. The engine's families are listed here with their owners; any other
family is a game's, declared by the game (Game codes, below), and a game may not declare a code in
an engine family (`game` among them: it is spec-arch's game thread).

| Family | Owner | About |
|---|---|---|
| `request` | this file | The request's shape: JSON, fields, types, ranges, names, aliases, versions. |
| `seat` | this file | Seats and the caller's right to them. |
| `permission` | this file | The caller's role. |
| `perception` | perception.md | Entities, facts, budgets, the omniscient view. |
| `action` | actions.md | Controls, intents' names, targets, affordances, conflicts. |
| `intent` | actions.md | Intent instances and their generic failures. |
| `time` | time.md | Pacing, turns, clocks, episodes. |
| `definition` | README | A game definition that fails validation at load. |
| `internal` | this file | Engine bugs. |
| `sim`, `rng`, `number` | spec-sim | Ticks, entities, events, physics colliders, RNG streams, numeric conversion (`docs/spec/simulation.md`, 11; `rng.md`, 9; `numeric.md`, 9). |
| `script`, `lint`, `types`, `scripts` | spec-script | Script load, type and run-time errors with the TypeScript file, line, tick and entity; the statelessness lint; `tsc` findings; hot-update requests (`docs/spec/script-sandbox.md`, 5.3; `hot-update.md`, 12). |
| `persist`, `replay`, `version`, `migrate` | spec-persist | Snapshots, restore, replay, versions, migrations (`docs/spec/persistence.md`, 11; `replay.md`, 4; `versions.md`, 9). |
| `queue`, `source`, `command`, `game` | spec-arch | The command queue and the game thread (`docs/spec/threads.md`, 5.4). |
| `lines`, `deps`, `fmt`, `clippy`, `build`, `gen`, `test`, `wasm`, `determinism`, `fork`, `reload`, `web`, `perf`, `check` | spec-arch | The local check command's findings (`docs/spec/checks.md`, 10.3); its `replay.*` and `types.*` codes belong to the families above, and its `contract.*` names are this contract's conformance checks (README). |
| `session` | spec-mcp | Sessions, tools, worlds and branches, the benchmark (`shared/contract/mcp.md`, 7.1). |

Every slice 0 specification spells its codes in this form; the integration of slice 0 settled the
early drafts' other spellings (upper-case `sim` codes, and spec-mcp's unprefixed ones:
`unknown_tool` is `request.unknown_method`, `not_permitted` is `permission.denied`,
`time.wrong_mode` or `session.policy_denies` by cause).

In the tables, "refuse", "warn" and "fail" are the uses above, and "warn (later)" a later warning.
`path` is always a JSON Pointer into the request; `suggestions` is at most three names, best first;
`allowed` is the full list of valid names at that place when there are at most 30 of them, otherwise
it is absent and `allowed_count` and `see` (the call that lists them, such as
`describe {"part": "intents"}`) stand in its place.

**Placeholders.** A template's placeholders are detail fields of its code, with four derived ones,
rendered the same way on all runtime targets:

- `{field}` is the last segment of `path` (`/actions/0/params/heading_deg` gives `heading_deg`; an
  array index gives `item <n>`).
- `{owner}` is the detail field `owner`: the intent's name for an intent's parameters
  (`come_to_heading`), `the <request> request` for a request's top level (`the act request`), and
  `actions[<i>]` for an action; `{parameter or field}` is `parameter` under an intent's `params` and
  `field` elsewhere.
- `{range}` renders `min`, `max` and `max_exclusive`: `from -1 to 1` (both inclusive),
  `from 0 up to but not including 360` (`max_exclusive`), `at least 1` or `at most 600` (one bound),
  each number written as the request's schema declares it.
- `{n}` is the number of `candidates`; a list (`{allowed}`, `{suggestions}`, `{candidates}`,
  `{seats}`, `{kinds}`) renders as `'a'`, `'a' or 'b'`, or `'a', 'b' or 'c'` for suggestions and as
  `a, b, c` otherwise; an entity (`{entity}`, `{target}`) renders as the text projection writes it,
  `Crate7#25`.

### `request`

| Code | Use | When | Detail | Message template |
|---|---|---|---|---|
| `request.malformed` | refuse | The body is not JSON, or not a JSON object. | `reason`, `offset` | `The request is not a JSON object: {reason} at byte {offset}.` |
| `request.unknown_method` | refuse | No request or tool of that name. | `method`, `suggestions`, `allowed` | `There is no request '{method}'; did you mean {suggestions}?` |
| `request.unknown_field` | refuse | A key the object does not take. | `path`, `field`, `owner`, `suggestions`, `allowed` | `{owner} has no {parameter or field} '{field}'; did you mean {suggestions}?` |
| `request.misplaced_field` | refuse | A key this object does not take that a neighbouring object does. | `path`, `field`, `belongs_at` | `'{field}' does not go here ({path}); it belongs at {belongs_at}.` |
| `request.missing_field` | refuse | A required key is absent. | `path`, `field`, `owner`, `expected` | `{owner} needs '{field}' ({expected}).` |
| `request.wrong_type` | refuse | A value of the wrong JSON type. | `path`, `expected`, `got` (the JSON type given) | `'{field}' must be {expected}; got {got}.` |
| `request.not_integer` | refuse | A number with a fraction where an integer goes. | `path`, `got` | `'{field}' must be a whole number; got {got}.` |
| `request.out_of_range` | refuse | A number outside its range, or a string or array of the wrong length. | `path`, `got`, `min`, `max`, `max_exclusive`, `hint` | `'{field}' must be {range}; got {got}.` plus the hint |
| `request.invalid_value` | refuse | A name not among an enumeration's values. | `path`, `got`, `allowed`, `suggestions` | `'{field}' must be one of {allowed}; got '{got}'.` |
| `request.conflict` | refuse | Fields that exclude each other, or an alias given with its own field. | `paths` | `{paths} cannot be given together.` |
| `request.not_applicable` | refuse | A field that does not apply in this combination. | `path`, `because` | `'{field}' does not apply: {because}.` |
| `request.ambiguous_ref` | refuse | A name that several entities the caller knows share. | `path`, `ref`, `candidates` (`[{id, name, kind}]`) | `'{ref}' names {n} entities; give one of their ids: {candidates}.` (each candidate as `Name#id`) |
| `request.unsupported_version` | refuse | `contract` names a version this engine does not serve. | `got`, `served` | `This engine serves contract {served}; the request asked for {got}.` |
| `request.alias_used` | warn | A declared alias was accepted (Aliases). | `path`, `alias`, `field` | `'{alias}' was read as '{field}'.` |

`{owner}` is what the object is, said as an agent would: `come_to_heading` for an intent's
parameters, `the act request`, `actions[0]` for an action. `{suggestions}` renders as `'a'`,
`'a' or 'b'`, or `'a', 'b' or 'c'`; with no suggestion the clause becomes `it takes {allowed}` (or
`see {see}`).

### `seat` and `permission`

| Code | Use | When | Detail | Message template |
|---|---|---|---|---|
| `seat.unknown` | refuse | No seat of that id. | `path`, `seat`, `suggestions`, `allowed` | `There is no seat '{seat}'; did you mean {suggestions}?` |
| `seat.not_yours` | refuse | A player names a seat other than its own. | `path`, `seat`, `yours` | `You play seat '{yours}', not '{seat}'.` |
| `seat.not_allowed` | refuse (as an unmet requirement) | An affordance limited to other seats. | `seat`, `seats` | `Only {seats} may do this.` |
| `permission.denied` | refuse | The caller's role may not make this request. | `request`, `role`, `needs` | `{request} needs the {needs} role; this caller is a {role}.` |

### `perception`

| Code | Use | When | Detail | Message template |
|---|---|---|---|---|
| `perception.unknown_entity` | refuse, fail | A reference to an entity the caller does not know, existing or not. | `path`, `ref`, `suggestions` | `No entity '{ref}' is known to you; did you mean {suggestions}?` |
| `perception.not_perceivable` | refuse | A fact or instrument the caller cannot know. | `path`, `name`, `entity` | `'{name}' is not something you can perceive.` |
| `perception.unknown_instrument` | refuse | No such instrument on the caller's profile. | `path`, `name`, `suggestions`, `allowed` | `There is no instrument '{name}'; did you mean {suggestions}?` |
| `perception.unknown_kind` | refuse | No such kind. | `path`, `kind`, `suggestions`, `allowed` | `There is no kind '{kind}'; did you mean {suggestions}?` |
| `perception.budget_too_small` | refuse | The mandatory part does not fit the budget. | `budget_tokens`, `min_tokens` | `A budget of {budget_tokens} tokens cannot hold this answer's header; give at least {min_tokens}.` |
| `perception.omniscient_forbidden` | refuse | A player asks for the omniscient view. | `role` | `The omniscient view is for developers and checkers, not players.` |
| `perception.cursor_ahead` | refuse | `since` is beyond the last perceived event. | `since`, `latest` | `No event {since} yet; the latest is {latest}.` |
| `perception.target_lost` | fail | An intent's target entity is no longer known to the seat. | `target` | `{target} is no longer known to you.` |

`perception.unknown_entity` is the answer for an entity that does not exist and for one that exists
but the caller does not perceive, remember or have on its chart, with the same message and detail,
and its suggestions are drawn only from names the caller knows (No leaks, below).

### `action` and `intent`

| Code | Use | When | Detail | Message template |
|---|---|---|---|---|
| `action.unknown_control` | refuse | No such control on the seat. | `path`, `seat`, `control`, `suggestions`, `allowed` | `Seat {seat} has no control '{control}'; did you mean {suggestions}?` |
| `action.unknown_intent` | refuse | No such intent for the seat. | `path`, `intent`, `suggestions`, `allowed` | `There is no intent '{intent}'; did you mean {suggestions}?` |
| `action.unknown_verb` | refuse | The entity's kind offers no such verb. | `path`, `entity`, `verb`, `suggestions`, `allowed` | `{entity} cannot be '{verb}'; it offers {allowed}.` |
| `action.target_required` | refuse | The intent needs a target and none was given. | `path`, `intent`, `takes` | `{intent} needs a target: {takes}.` |
| `action.target_not_taken` | refuse | A target given to an intent that takes none. | `path`, `intent` | `{intent} takes no target.` |
| `action.wrong_target_kind` | refuse | The target's kind is not one the intent takes. | `path`, `intent`, `target`, `kind`, `kinds` | `{intent} cannot target {target}, a {kind}; it takes {kinds}.` |
| `action.conflict` | refuse | Two parts of one call drive the same channel or control. | `paths`, `channel`, `control` | `{paths} both drive {channel or control}; send them in separate calls.` |
| `action.unavailable` | refuse | An affordance whose requirements are not all met. | `path`, `entity`, `verb`, `unmet` (problems) | `{entity} cannot be '{verb}' now: {first unmet message}` |
| `action.not_seen` | refuse (unmet) | The entity is not seen now. | `entity` | `{entity} is not in sight.` |
| `action.out_of_reach` | refuse (unmet) | The entity is too far. | `entity`, `range_m`, `max_m` | `{entity} is {range_m} m away; it must be within {max_m} m.` |
| `action.not_facing` | refuse (unmet) | The entity is too far off the heading. | `entity`, `off_deg`, `max_off_deg` | `{entity} is {off_deg} degrees off the bow; it must be within {max_off_deg}.` |
| `action.requirement_unmet` | refuse (unmet) | A declared fact requirement fails. | `verb`, `of`, `fact`, `op`, `value`, `actual` | `{verb} needs {of}'s {fact} {op} {value}; it is {actual}.` |
| `action.dropped` | warn (later) | A lockstep action that was valid when submitted failed at its boundary. | `tick`, `action`, `problem` | `Your action for tick {tick} was dropped: {problem message}` |
| `intent.unknown_id` | refuse | `cancel`, `intents {ids}` or `until {"intent": n}` naming an id this seat never had (or that was pruned). | `path`, `intent_id` | `You have no intent #{intent_id}.` |
| `intent.timeout` | fail | The deadline passed while active. | `intent`, `timeout_s` | `{intent} did not finish within {timeout_s} s.` |

### `time`, `definition` and `internal`

| Code | Use | When | Detail | Message template |
|---|---|---|---|---|
| `time.wrong_mode` | refuse | A request the current pacing or structure does not take. | `request`, `pacing`, `structure`, `takes` | `{request} is not available in {pacing} pacing; use {takes}.` |
| `time.not_clock_holder` | refuse | `step` from someone who does not hold the clock. | `holder` | `Only {holder} steps this session.` |
| `time.not_your_turn` | refuse | An action or `end_turn` outside the seat's turn. | `turn`, `to_move` | `It is {to_move}'s turn ({turn}).` |
| `time.episode_over` | refuse | A time request or an `act` after the episode ended. | `outcome` | `The episode ended at tick {outcome.tick}.` |
| `time.busy` | refuse | A `step` while another clock holder's `step` runs on the same world. | `tick` | `Another step is running (at tick {tick}); wait for it to answer.` |
| `time.halted` | halt (players) | The simulation halted after a script error; the player's form of a halt. | | `The game stopped on an internal error; only a developer can resume it.` |
| `time.desync` | refuse | Lockstep peers' tick hashes differ; the match stops. | `tick`, `peers` | `The peers' games diverged at tick {tick}; the match stopped.` |
| `time.cannot_pause` | refuse | `pause` or `resume` from a seat not allowed to. | `seat` | `Seat {seat} may not pause the game.` |
| `time.no_decision` | warn | `continue` with no pending decision. | | `There was no decision to answer.` |
| `time.clock_out` | warn (later) | The seat's thinking clock ran out before it answered. | `decision`, `tick` | `Your thinking time ran out at tick {tick}; the game went on.` |
| `definition.invalid` | refuse (game load) | The game definition fails validation (README, The game definition). | `path` (into the definition), `reason` | `The game definition is invalid at {path}: {reason}.` |
| `internal.error` | refuse, fail | A bug: a handler failed or an executor broke its contract. | `where`, `report` | `The engine failed in {where}; this is a bug, reported as {report}.` |

An internal error never takes the session down: the call that hit it is refused (or the intent
fails), and the engine keeps serving; master's first benchmark failure was a runtime that exited on
a script error under the harness (master `docs/agent-eval.md`, The first failure was the engine's).

## Unknown names and suggestions

Every place a request names something (a field, a parameter, a method, a seat, a control, an intent,
a verb, a kind, an instrument, an enumeration value, an entity's name) is checked against what is
valid there, and an unknown name is refused with suggestions. Suggestions are computed the same way
everywhere, deterministically, so the runtime targets give the same ones.

**Normalization** of a name `s`: insert `_` between a lowercase letter or digit and a following
uppercase letter (`headingDeg` to `heading_Deg`); lowercase ASCII; replace `-`, space and `.` with
`_`; collapse runs of `_`; trim `_` at both ends. **Stem** of a normalized name: the name without a
trailing unit suffix (`_m`, `_mps`, `_deg`, `_s`, `_kg`; README, Names), or the name itself.

For the unknown name `u` and each valid name `c` (and each declared alias of `c`, which suggests
`c`), with `nu`, `nc` their normalizations:

1. **Score 0** when `nu == nc`: the same name in another case or separator style.
2. **Score 1** when `nu == stem(nc)` or `stem(nu) == stem(nc)`: the unit suffix left off or wrong
   (`heading` for `heading_deg`, `speed_m` for `speed_mps`).
3. **Score 2** when `d = min(osa(nu, nc), osa(nu, stem(nc)))` is at most `max(1, len(nu) / 3)`
   (integer division), where `osa` is the optimal-string-alignment edit distance
   (`strsim::osa_distance`, strsim 0.11.1): a typo. rustc's threshold for "a field with a similar
   name exists" is the same third of the length.
4. **Score 3** when `nu` and `nc` share a `_`-separated word of three or more letters that is not a
   unit suffix (`deg`, `mps`): a related name (`sail_toward` for `sail_to` and `trim_sail`).

The suggestions are the scored valid names, ordered by score, then by `d` (for scores 2 and 3;
`osa(nu, nc)` for 3), then by name in byte order, without duplicates, at most three. Entity names
are compared the same way, but only among entities the caller knows.

**Misplaced fields.** When an unknown key `u` scores 0 or 1 nowhere at its own place but does at a
place one level away in the same request (a child object such as an action's `params`, or the parent
object), the problem is `request.misplaced_field` with `belongs_at`, the JSON Pointer where it would
be valid. This catches the commonest structural slip, an intent parameter written beside `intent`
instead of under `params`, and a single action sent without the `actions` array.

**Enumeration values** get the same treatment with `request.invalid_value`: `"Port"` suggests
`port`, `"best_trim"` suggests `best`.

## Aliases

An alias is another name the contract deliberately accepts for a field or a name, because agents
reach for it. It is declared, never inferred: master accepted any field named by its only
three-letter prefix (master `docs/mcp.md`, Asking the engine how to call it), which meant adding a
field could silently change what an old call meant, and that is not carried over.

- A field whose name ends in a unit suffix accepts its stem as an alias (`heading` for
  `heading_deg`), unless the stem is itself a valid name in the same object or the stem of another
  field there.
- Any other alias is declared in the schema, as the JSON Schema extension keyword `x-aliases`
  (schemars: `#[schemars(extend("x-aliases" = ["..."]))]`), on a field, a control, an intent, a verb
  or an enumeration value. Each declared alias cites the recorded agent failure that asked for it in
  the contract's version table (README, Versioning), as master's aliases came one by one from failed
  calls in benchmark traces.
- An accepted alias is reported with the warning `request.alias_used`, and the canonical name is
  what the engine applies, answers and records (actions.md, What a replay records).
- An alias and its own field in the same object is `request.conflict`.

## Several problems

A call is validated in phases (actions.md, Validation and atomicity, for actions; other requests
have only the first two): the request's shape, then the names it uses, then the semantics.
Validation stops after the first phase that finds a problem and reports every problem that phase
found:

- The problems are ordered by `path` (JSON Pointer segments compared one by one, array indices as
  numbers, keys in byte order; a problem without a path last), then by code.
- The first is the answer's problem. The rest, at most nine, are in its `detail.also`, each a whole
  problem without an `also` of its own; `detail.also_more` counts any beyond those.
- `also`, `also_more` and `path` are reserved detail keys; no code uses them for anything else.

So an agent that misspelt two parameters learns of both in one answer, and one that misspelt a
parameter and named an unknown mark learns of the spelling first, because the names are not checked
until the shape is right.

## Messages

- One or two sentences in English, at most 300 bytes, ending with a full stop or a question mark.
- They name what the caller wrote (the field, the name, the value), what is wrong, and what would be
  right: the suggestion, the range, or the valid names when there are at most eight.
- No Rust type names, no stack traces, no file paths of the engine, no memory addresses: master's
  agents read the engine's source when an answer did not say enough (master `docs/agent-eval.md`,
  Watching the agents changed the interface), and the message is where that ends.
- Numbers in messages are written as the text projection writes them, at the precision of the fact,
  instrument or parameter they report (projection.md, Rounding); a value quoted from the request is
  written as the request had it.
- A game's codes declare their templates the same way (Game codes); the engine renders them.

## No leaks

A problem a player receives says nothing the player cannot perceive. An entity it does not know is
answered exactly as a nonexistent one; suggestions and `candidates` list only names it knows; unmet
requirements carry the values it perceives (a range to a crate it sees, not one it does not); a
refused `until` on a hidden fact says only that the fact is not perceivable. The omniscient view's
answers, for developers and checkers, carry everything, marked (perception.md, The omniscient view).

## Game codes

A game declares its own codes in its definition (README, The game definition):

```rust
pub struct CodeDef {
    pub code: String,                   // "sail.aground": a family the game owns
    pub doc: String,
    pub uses: Vec<Use>,                 // refuse, warn, fail
    pub detail: Vec<FactDef>,           // its detail fields, with units and precision
    pub message: String,                // template over the detail fields
}

pub enum Use { Refuse, Warn, Fail }
```

The sailing game's codes are in [sailing.md](sailing.md), Codes.

## Determinism

Problems are deterministic: the same request against the same world and caller gives the same code,
detail and message bytes on all runtime targets. Validation reads the request, the caller's perception and
the game definition only; suggestion order is total; numbers are rounded by the shared rule. No
problem carries a wall-clock time, a memory address or a hash-map iteration order. Refusals change
nothing (actions.md, Validation and atomicity), so a refused call cannot make two runs diverge.

## Checks

- **`contract.errors.golden`**: shared cases in `shared/contract/conformance/errors.jsonl` against
  the sailing showcase at a fixed tick, each a request, the caller's role and seat, and the expected
  problem (code, detail, message). All runtime targets must give them byte for byte. The first cases:

| Request (abridged) | Expected |
|---|---|
| `start come_to_heading {"headng": 90}` | `request.unknown_field`, `/actions/0/params/headng`, suggestions `["heading_deg"]` |
| `start come_to_heading {"heading": 90}` | accepted; warning `request.alias_used`, `heading` read as `heading_deg` |
| `start come_to_heading {"headingDeg": 90}` | `request.unknown_field`, suggestions `["heading_deg"]` (score 0, not an alias) |
| `{"do": "start", "intent": "come_to_heading", "heading_deg": 90}` | `request.misplaced_field`, `belongs_at` `/actions/0/params/heading_deg` |
| `start come_to_heading {"heading_deg": 360}` | `request.out_of_range`, `min` 0, `max_exclusive` 360, hint `360 is 0`; message `'heading_deg' must be from 0 up to but not including 360; got 360. 360 is 0.` |
| `start come_to_heading {"heading_deg": "90"}` | `request.wrong_type`, expected `number`, got `string` |
| `start come_to_heading {"heading_deg": 90, "turn": "left"}` | `request.invalid_value`, allowed `["shortest", "port", "starboard"]` |
| `start sail_toward` | `action.unknown_intent`, suggestions `["sail_to", "trim_sail"]` |
| `start come_to` | `action.unknown_intent`, suggestions `["come_to_heading"]` |
| `set {"ruder": 0.2}` | `action.unknown_control`, suggestions `["rudder"]` |
| `set {"tiller": 0.2}` | `action.unknown_control`, suggestions `[]`, allowed `["rudder", "sheet", "hoist", "interact"]`; message `Seat skipper has no control 'tiller'; it takes rudder, sheet, hoist, interact.` |
| `set {"rudder": 1.5}` | `request.out_of_range`, `/actions/0/controls/rudder`, `min` -1, `max` 1 |
| `set rudder` and `start come_to_heading` in one call | `action.conflict`, channel `helm` |
| `start sail_to` target `Mark9` (no such mark) | `perception.unknown_entity`, suggestions `["Mark1"]` |
| `start sail_to` target a mark that exists behind an island, not on the chart | the same problem as `Mark9`, byte for byte apart from `ref` |
| `start sail_to` target `"Mark1#40"`; target `{"id": 40, "name": "Mark1"}`; target `"#40"` | each accepted as entity 40 |
| `start sail_to` target `"Mark2#40"` (40 is Mark1) | `perception.unknown_entity` |
| `cancel {"intent_id": "#3"}` | accepted as intent 3 |
| `intents {}` after a `sail_to` ended in `sail.no_progress` | the intent with `failure` `{code: "sail.no_progress", message, detail: {distance_m, needed_m, window_s}}` |
| `use Crate7 take_aboard` at 14.2 m | `action.unavailable`, `unmet` `[action.out_of_reach {range_m: 14.2, max_m: 3}]` |
| `start trim_sail {}` with the sail furled | `sail.not_set` |
| `observe {"budget_tokens": 10}` | `perception.budget_too_small`, `min_tokens` the mandatory part's size |
| player `observe {"omniscient": true}` | `perception.omniscient_forbidden` |
| player `act {"seat": "rival", ...}` | `seat.not_yours` |
| `step` in real-time pacing | `time.wrong_mode`, takes `["wait", "continue", "pause", "resume"]` |
| `{"contract": "1.0", ...}` | `request.unsupported_version` |
| `heading` and `heading_deg` together | `request.conflict` |
| two misspelt parameters | the first by path, the second in `detail.also` |
| request `observ` | `request.unknown_method`, suggestions `["observe"]` |

- **`contract.actions.refusal`** (actions.md, Checks) covers every field of every request type with
  generated misspellings.
- **Template renderings**: `contract.errors.golden` holds a case for each derived placeholder
  (`{field}` from a nested path and from an array index, each form of `{range}`, `{n}` with two and
  three candidates, `{owner}` for an intent's parameters, a request and an action) and for every
  code whose template names `{seat}`, `{intent}` or `{verb}`.
- **Suggestion unit tests**: each scoring rule, its thresholds at the boundary (a distance of
  exactly `len / 3` suggests, one more does not), the ordering ties, normalization of camelCase,
  kebab-case and dots, and the three-suggestion cap.

The suggestions in the table were computed with a Python prototype of the algorithm above (a scratch
script of this task, not part of the repository), which also gave `heading_rad` to `heading_deg`
(score 2), `mark1` to `Mark1` (score 0), `Port` to `port`, `best_trim` to `best` (score 3), and
nothing for `left` among the `turn` values or `steer` among the controls; the implementations in
all runtime targets must reproduce them.

## Open choices

1. **One problem with `also`, or a list.** Recommended: one problem at the top (the charter's
   `{code, message, detail}`), the rest in `detail.also`, so every consumer handles one shape. The
   alternative, an array of problems as the error, changes the shape by count.
2. **Suggestion algorithm.** Recommended: the four scores above with strsim's OSA distance.
   Jaro-Winkler (clap's choice) favours shared prefixes and suggests more for short names; the
   shared golden cases fix whichever is chosen.
3. **Synonym aliases** (`left` for `port`, `tiller` for `rudder`). Recommended: only when a recorded
   agent failure asks for one, as declared aliases. Until then the refusal lists the valid names,
   which costs one round trip and teaches the vocabulary.
4. **Integer `error.code` numbers for JSON-RPC.** Recommended: the two JSON-RPC codes above, with
   the contract's code in `data`; the string code is the one that matters.
5. **Slice 1 (Amoris): how `pocket-contract` implements this file.**
   - One constructor per code of the contract's families in `pocket_contract::codes`, each rendering
     the template of the tables above through one renderer (`render::render`) that game codes use
     too (`Problem::from_template`); `CodeDef` waits for perception.md's `FactDef`, which the crate
     does not declare yet. Detail numbers that are integral doubles below 2^53 are written as JSON
     integers (`max_m: 3`, not `3.0`), so details compare equal on all runtime targets.
   - With no suggestion, `did you mean {suggestions}?` becomes `it takes {allowed}.` only for at
     most eight names (Messages); with more, `see {see}.` when the caller names where to look, else
     `it takes one of N names, listed in the detail.`; with neither list nor `see` (an unknown
     entity, No leaks) the clause is dropped. Messages longer than 300 bytes are cut.
   - The request checker (`pocket_contract::shape`) works from the request type's JSON Schema as
     schemars writes it. It refuses a key no property takes even where the schema omits
     `additionalProperties: false`; it accepts unit-stem and `x-aliases` aliases with
     `request.alias_used`; it reports `request.missing_field` for a required field only when no
     unknown key of the same object stands for it (as its first suggestion or by belonging under
     it), so `{"headng": 90}` answers with the one mistake; it reads integer ranges from schemars'
     `format` (`uint32` is 0 to 4294967295, 64-bit integers ±(2^53 - 1)); it reports a string or
     list of the wrong length as `request.out_of_range` with `got` the length and the hint
     `That is its length.`; and a typed decode that fails after the check passed is
     `internal.error`, since schema and decoder then disagree. Hints that need the field's meaning
     (`360 is 0.`) are the caller's.
   - `allowed` lists drawn from a schema come in byte order, the order `serde_json`'s maps keep
     (persistence.md forbids `preserve_order`); the tables above show declaration order, which
     callers that pass their own lists (`codes::unknown_control`, ...) keep.
