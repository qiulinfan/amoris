# Replays and divergence

Status: Draft, slice 0

Charter: 3.3 (replay MUST name the first tick where two runs diverge), 3.7 (the replay and fork
checks), 4.2.6 (restore to any tick; one recording replayed with old and new scripts), 5.1 (the
browser-local form watches replays), 7.5 (the byte format of replays), 7.13 (replays record the
script bundle hash and engine version).

This specification fixes the replay file, the recorder, reading, seeking and the session history,
how a restore or a reset appears in a recording, and how two runs are compared to name the first
tick where they diverge and the component that diverged. It was split from
[persistence.md](persistence.md), whose canonical encoding (PCE, 3), snapshots (4 and 6), world hash
(5), errors (11) and checks (12) it uses. Versions, the modes that gate a replay, embedded bundles
and migrations are [versions.md](versions.md).

## 1. Related specifications

| Owner | Concepts used here | File |
|---|---|---|
| spec-persist | PCE, `Snapshot`, `SectionKey`, `WorldHash`, `TickHash`, `ContentHash`, `restore`, `fork`, resolved formats, `EngineVersion`, `Bundle`, format tables, run configuration, `ReplayMode` and `verify`'s options | [persistence.md](persistence.md), [versions.md](versions.md) |
| spec-sim | boundaries and boundary writes, a point `(tick, writes)`, the poisoned world | [simulation.md](simulation.md) |
| spec-script | `CompiledSet` and instantiation, faults, the debugger | [script-host.md](script-host.md), [script-sandbox.md](script-sandbox.md), [hot-update.md](hot-update.md) |
| spec-arch | `Applied`, `Source`, command kinds, the canonical recorded form of a Write | [threads.md](threads.md) |
| spec-contract | `Problem`; the session-level checks a replay skips | [shared/contract/](../../shared/contract/README.md) |
| spec-mcp | the `replay`, `restore` and `reset` tools and branches | [mcp.md](../../shared/contract/mcp.md) |

The code is `pocket-persist` (architecture.md 4.3).

## 2. Replays

### 2.1 Contents

A replay is a start snapshot plus everything applied afterwards, with each tick's `TickHash`. It
does not depend on wall-clock time: pacing, pauses and thinking budgets decide at which boundary a
write lands (simulation.md 3.3), and the replay records that boundary. The tick record of tick `t`
holds the writes applied at boundary `t - 1`, in application order, and the `TickHash` after tick
`t`; this is `state(t) = tick_t(apply(state(t-1), writes(t)))` of simulation.md 4.2.

The writes are what `threads.md` 5.3 hands the recorder as `Applied`: every successful Write in its
canonical recorded form (threads.md 5.3: after validation, every `EntityRef` resolved to an
`EntityId`, aliases resolved and defaults filled), from Host, Editor, Developer and Player sources,
hot updates and imports included; data that arrived off-thread is named by hash and carried by a
`Data` record (2.2). A restore or a reset is not a write inside a segment: it ends one (2.5). Acts a
script makes inside a tick (`ctx.intent`) are outputs of that tick and are never recorded
(shared/contract/actions.md, What a replay records). Each write is stored as:

```rust
pub struct RecordedWrite {
    pub source: Source,     // threads.md 5.1
    pub seq: u64,
    pub name: String,       // the command's catalog name
    pub params: String,     // serde_json compact text of the canonical params (below)
}
```

`params` is `serde_json::to_string` of the canonical value: keys sorted (no `preserve_order`),
integers exact, floats in shortest round-trip form with the sign of zero kept, read back with
`float_roundtrip` to the same bits; JSON stays readable by agents and stable across Rust type
changes. Refused commands changed nothing and are kept only as notes.

Replaying a write (`Stepper::apply`, 2.4) runs the live application path: decoding, world-level
validation (names, entity ids, values, ranges, conflicts, affordance requirements, an intent's
`accept`, which recomputes the instance's initial `state` and `progress`) and application. Only
session-level checks are skipped, since a replay has no session and the recorded `source` stands for
them: the caller's role, its grant and policy, and whether it holds the clock. A world-level refusal
on replay is `replay.write_refused` in `verify` (3.4) and `DivergenceKind::Write` in `lockstep`
(3.3); it means the replaying engine or scripts disagree with the recording.

### 2.2 File layout

```text
replay = "P3DRPLY" 0x00  format:u32-LE (1)  ULEB128 length, PCE(ReplayHeader)  record*
record = tag:u8  length:ULEB128  payload (PCE)
```

| Tag | Record | Written |
|---|---|---|
| `0x01` | `Start { snapshot }` | Once, first: opens segment 0 |
| `0x02` | `Formats { table }` (versions.md 4) | After `Start` and each `Rebase`; again when a hot update registers a project component |
| `0x03` | `Bundle(BundleRecord)` | Before the first record that loads it (versions.md 7.4); each segment's bundle right after its `Start` or `Rebase` when it was not recorded before |
| `0x04` | `Tick { tick, writes: Vec<RecordedWrite>, hash: TickHash, digests: DigestDelta }` | Every tick that completes |
| `0x05` | `Keyframe { snapshot }` | After `Tick(t)` when `t - t0` is a multiple of `keyframe_every`, `t0` the segment's start |
| `0x06` | `DataRead { path, hash: ContentHash }` | When the game thread reads a simulation data file (versions.md 3.3) |
| `0x07` | `Refused { write, code }` | A refused command (not replayed) |
| `0x08` | `End { tick, writes: Vec<RecordedWrite>, world: WorldHash }` | When a segment ends normally: at finish, and before a `Rebase`; the writes applied at the last boundary and the hash after them |
| `0x09` | `Rebase { cause: RebaseCause, snapshot }` | After `End`, when a restore or a reset replaces the world: opens the next segment (2.5) |
| `0x0A` | `Data { hash: ContentHash, bytes }` | With `embed_data`, once per hash, before the first `DataRead` or write that names it |
| `0x0B` | `Fault { tick, system, code }` | Instead of `Tick(t)` when a fault poisons the world in tick `t` (script-sandbox.md 4.3); it ends the segment |
| `0x0C` | `Tainted { tick, reason }` | Once, at the first debugger evaluation, condition or variable write inside a tick (script-host.md 13) |

```rust
pub struct ReplayHeader {
    pub engine: EngineVersion,           // versions.md 3.1
    pub start_tick: Tick,
    pub seed: u64,                       // world seed and tick rate at the start (rng.md 5.4,
    pub tick_rate: u32,                  // simulation.md 3.1), copied for reports
    pub run_config: String,              // canonical JSON run configuration (versions.md 3.7)
    pub keyframe_every: u32,             // 0: none
    pub digests: DigestLevel,            // World or Sections
    pub label: BTreeMap<String, String>, // scenario, agent, model: free metadata
    pub parent: Option<ParentRef>,       // the run this was forked from
}
pub enum DigestLevel { World, Sections }
pub struct ParentRef { pub replay: Option<ContentHash>, pub tick: Tick, pub world: WorldHash }
pub enum RebaseCause { Restore, Reset }

/// A bundle as a replay carries it. `files` are the TypeScript sources the hash covers
/// (versions.md 3.2); `compiled` is what the recording engine's transpiler made of them.
pub struct BundleRecord {
    pub hash: ContentHash,
    pub files: Vec<(String, Vec<u8>)>,          // path order
    pub compiled: Option<CompiledRecord>,       // with `embed_compiled` (default true)
}
pub struct CompiledRecord {
    pub engine_source: ContentHash,             // EngineVersion.source of the engine that compiled it
    pub modules: Vec<CompiledModuleRecord>,     // path order; the prelude is engine code, not here
}
pub struct CompiledModuleRecord { pub path: String, pub js: String, pub map: String }

/// Section digests that changed since the previous tick of the segment.
pub struct DigestDelta { pub added: Vec<SectionKey>, pub removed: Vec<u32>,
                         pub changed: Vec<(u32, SectionDigest)> }
```

**Section digests.** A segment keeps a running list of its world's sections in canonical section
order (persistence.md 4.1), starting as the sections of its `Start` or `Rebase` snapshot. A
`DigestDelta` is applied to it in this order: remove the `removed` indices, all taken relative to
the previous list, and compact the list; insert the `added` keys at their places in canonical order;
then apply `changed`, whose indices are relative to the list that results. A section that appears is
in `added` and its digest in `changed`. So two readers of one file rebuild the same list at every
tick and name the same diverging sections.

Records are written in the order things happen; a `Tick` closes its tick. The file is append-only: a
recording cut short (a crash) replays up to its last complete `Tick`, and the reader reports
`complete: false`. An unknown tag is `replay.format`; adding a record kind bumps the format.
Extensions: `.p3dreplay`, and `.p3dsave` for saves (versions.md 6).

### 2.3 Recording

```rust
pub struct RecordOptions {
    pub keyframe_every: u32,             // default 600 (persistence.md 13)
    pub digests: DigestLevel,            // default Sections
    pub embed_bundles: bool,             // default true: TypeScript sources (versions.md 7.4)
    pub embed_compiled: bool,            // default true: the compiled modules beside them
    pub embed_data: bool,                // default true: the bytes of every data file named
    pub label: BTreeMap<String, String>,
    pub parent: Option<ParentRef>,
}
pub trait RecordSink: Send {
    fn write(&mut self, record: &[u8]) -> std::io::Result<()>;
    fn flush(&mut self) -> std::io::Result<()>;
}
impl Recorder {
    pub fn start(world: &World, reg: &Registry, bundle: &BundleRecord, run_config: &str,
                 opts: RecordOptions, sink: Box<dyn RecordSink>) -> Result<Recorder, Problem>;
    pub fn applied(&mut self, write: &Applied);              // threads.md 5.3, in order
    pub fn refused(&mut self, write: &Applied, code: &str);
    pub fn data_read(&mut self, path: &str, hash: ContentHash, bytes: &[u8]);
    pub fn data(&mut self, hash: ContentHash, bytes: &[u8]); // an import's result, before its write
    pub fn bundle_loaded(&mut self, bundle: &BundleRecord); // before the write that swaps it
    pub fn end_tick(&mut self, world: &World) -> Result<TickHash, Problem>;
    pub fn rebase(&mut self, world: &World, cause: RebaseCause, bundle: &BundleRecord)
        -> Result<(), Problem>;                              // 2.5
    pub fn fault(&mut self, tick: Tick, system: &str, code: &str);
    pub fn tainted(&mut self, tick: Tick, reason: &str);
    pub fn finish(self, world: &World) -> Result<ReplaySummary, Problem>;
}
```

The recorder flushes after every keyframe, at each `End` and at `finish`. Two sinks are required: a
file (native) and memory (web, checks, and the history of 2.4). `Bundle` and `CompiledSet` are
spec-script's (versions.md 3.2; script-host.md 8.2); a `BundleRecord` is built from the loaded
`Program`'s bundle and its compiled set.

### 2.4 Reading, seeking and history

```rust
/// A tick of one segment: ticks restart, and may repeat, after a Rebase.
pub struct TickRef { pub segment: u32, pub tick: Tick }
pub struct SegmentInfo { pub index: u32, pub cause: Option<RebaseCause>, // None for segment 0
                         pub start: (Tick, u32), pub last: Tick, pub end: SegmentEnd }
pub enum SegmentEnd { End, Fault { tick: Tick, code: String }, Open }    // Open: cut short

impl Replay {
    pub fn read(bytes: &[u8]) -> Result<Replay, Problem>;   // replay.format, replay.truncated
    pub fn header(&self) -> &ReplayHeader;  pub fn complete(&self) -> bool;
    pub fn segments(&self) -> &[SegmentInfo];
    pub fn ticks(&self, segment: u32) -> RangeInclusive<Tick>;
    pub fn hash(&self, at: TickRef) -> Option<TickHash>;
    pub fn writes(&self, at: TickRef) -> &[RecordedWrite];   // applied at boundary tick - 1
    pub fn keyframe_at_or_before(&self, at: TickRef) -> &Snapshot; // the segment's start when none
    pub fn bundle(&self, h: &ContentHash) -> Option<&BundleRecord>;
    pub fn data(&self, h: &ContentHash) -> Option<&[u8]>;
    pub fn tainted(&self) -> Option<TickRef>;
}

/// The game loop as replay code drives it; implemented by pocket-runtime, which owns the world,
/// the script host and the command application (architecture.md 4.8, 9 item 2).
pub trait Stepper {
    fn world(&self) -> &World;
    fn registry(&self) -> &Registry;
    fn restore(&mut self, snap: &Snapshot, bundle: &BundleRecord) -> Result<(), Problem>;
    fn apply(&mut self, write: &RecordedWrite, source: &dyn ReplaySource)
        -> Result<(), Problem>;                         // the live path, 2.1
    fn step(&mut self) -> Result<Vec<Event>, Problem>;  // one tick; its event record
}
/// Where a stepper finds the bundles and data a write or a DataRead names: the replay's own
/// records first, then substitutes, then a content store located by hash.
pub trait ReplaySource {
    fn bundle(&self, h: &ContentHash) -> Option<BundleRecord>;
    fn data(&self, h: &ContentHash) -> Option<Arc<[u8]>>;
}

pub enum SeekScripts { Current, Recorded }                  // versions.md 7.5
pub struct SeekReport { pub at: TickRef, pub world: WorldHash, pub resimulated: u64,
                        pub scripts_changed_since: bool }
pub fn seek(replay: &Replay, s: &mut dyn Stepper, at: TickRef, scripts: SeekScripts)
    -> Result<SeekReport, Problem>;                     // replay.out_of_range
```

**Instantiating a recorded bundle.** A stepper that must load a `BundleRecord` (at a start, a
`Rebase`, or a recorded `scripts.swap`) instantiates its embedded JavaScript when
`compiled.engine_source` equals the running engine's `EngineVersion.source`, whatever the build's
features; otherwise it compiles the TypeScript, which needs the `transpile` feature (architecture.md
7.4). A build without `transpile` refuses every other case with
`replay.bundle_unavailable {hash, reason: "needs transpile"}`. So the shipped web worker, which has
no transpiler, verifies a natively recorded replay from the same source tree (checks.md 7.2,
`replay`), and a browser given only the file can watch it (versions.md 7.4).

**Data.** A `DataRead` or an asset write names a hash; the stepper takes the bytes from the
recording's `Data` record, else from the content store (the project's files and the asset cache,
located by hash), else stops with `replay.data_unavailable {hash, path}`. A recording made with
`embed_data: false` therefore replays only where the content store holds every hash it names.

`seek` restores the nearest keyframe at or before `at` in its segment, re-applies the recorded
writes and ticks with the recorded bundles, checking each `TickHash`, then loads the scripts
`scripts` names. "Restore to any tick" (charter 4.2.6) is `seek` over the running recorder's memory
sink: a session's history is its own replay, segments included. The history keeps the start, every
tick record, and keyframes thinned with age (all within the last 3,600 ticks, exponentially fewer
before), the "exponentially spaced snapshots" master adopted from its survey (master
`docs/design/agent-perception.md`, Adopted from the September 2026 survey); a seek further back
re-simulates longer, never wrongly.

### 2.5 Restore, reset and segments

A restore (of a session snapshot, or of a branch adopted as main, shared/contract/mcp.md 5.6) or a
reset (mcp.md 6) replaces the whole world and may move its tick backwards, and what it names ("s1",
"b1") is session state no file holds. So neither is recorded as a write. The recorder writes `End`
for the world as it was (none after a `Fault`, which already ended the segment), then
`Rebase { cause, snapshot }` holding the full snapshot the world now holds (after any migration,
versions.md 7.5), then `Formats`, then the `Bundle` the world now runs when it was not recorded
before. `Tick` records continue from that snapshot's `(tick, writes)`.

A recording is therefore a sequence of segments, each a start snapshot and the ticks run from it:
segment 0 opens with `Start`, each later one with `Rebase`. Reading, seeking, verifying and
comparing work per segment and name a tick as a `TickRef`. A `Fault` record also ends a segment; a
poisoned world runs no tick until a restore opens the next one (simulation.md 4.6).

Each branch (shared/contract/mcp.md 5.5) has a recording of its own, which opens with its `Start`
(the fork's snapshot, `parent` set to the source run and tick) and the `Bundle` record of the bundle
it inherited at the fork (versions.md 7.5); a swap into the branch is recorded there, never in its
source's.

## 3. Divergence

### 3.1 The first diverging tick

Two runs diverge at tick `t` when their `TickHash`es after `t` differ and those after `t - 1` were
equal. A tick hash does not chain earlier ticks, so a difference that later disappears is reported
with both facts. Master's per-tick hash was a chain and its run report kept the last eight tick
hashes, so a divergence could be detected but not placed (master `engine/app/src/session.cpp`,
`tick_hash_tail`; Amoris charter 8.2); a replay here keeps every tick's hash.

Two recordings are compared segment by segment, paired by index: a segment's start snapshot hash is
compared first (a difference is a divergence at the segment's first tick, kind `Hash`), then its
ticks. A run with fewer segments ends early (`EndedEarly`).

```rust
pub enum DivergenceKind {
    Hash,                                       // the tick hashes differ
    Write { index: u32, refused_by: Side },     // one side refused a write the other applied
    EndedEarly { side: Side },
    Stopped { error: Problem },                 // a fault, a taint, a missing bundle or data file
}
pub enum Side { Expected, Actual }
pub struct Divergence {
    pub at: TickRef,
    pub kind: DivergenceKind,
    pub expected: Option<TickHash>, pub actual: Option<TickHash>,
    pub sections: Vec<SectionKey>,      // differing sections; empty when unknown (3.2)
    pub fields: Vec<FieldDiff>,         // empty when unavailable
    pub fields_truncated: bool,
    pub writes: Vec<RecordedWrite>,     // applied at boundary tick - 1
    pub reconverged_at: Option<TickRef>,
}
pub fn first_divergence(expected: &[(TickRef, TickHash)], actual: &[(TickRef, TickHash)])
    -> Option<Divergence>;
```

### 3.2 Which component diverged

| Both sides offer | The report names |
|---|---|
| Tick hashes only | The tick |
| Section digests (a live world, or a replay with `DigestLevel::Sections`) | The tick and the differing sections: component types, resources, caches |
| Full state (two live worlds, a keyframe on that tick, or a child process asked for its snapshot, checks.md 8.2) | The tick, the sections, and each differing entity, component and field with both values |

```rust
pub struct FieldDiff {
    pub section: SectionKey,
    pub entity: Option<EntityId>,         // None for resources and the entities section
    pub path: String,                     // RFC 6901 JSON Pointer into the value; "" for all of it
    pub expected: serde_json::Value,      // Null when the row or field is absent
    pub actual: serde_json::Value,
    pub bits: Option<(String, String)>,   // hex bit patterns when the JSON renders are equal
}
pub struct SnapshotDiff { pub sections: Vec<SectionChange>, pub fields: Vec<FieldDiff>,
                          pub truncated: bool }
pub enum SectionChange { OnlyIn { side: Side, key: SectionKey }, Differs { key: SectionKey } }
```

`diff` (persistence.md 6.3) compares section digests and decodes only the sections that differ. In a
component section it merges the two row lists by `EntityId` (a row on one side only is a whole-value
difference); in a row it walks both values with the type's resolved format (versions.md 3.5),
comparing leaves by their bytes, so `-0.0` against `+0.0` and different NaNs are found where JSON
equality would miss them. Each differing leaf is a `FieldDiff` with a JSON Pointer path
(`/position/x`) and both values as JSON. Caches appear only in `sections`. `limit` caps `fields`
(default 50).

### 3.3 Lockstep

```rust
pub struct LockstepOptions { pub until: Tick, pub field_limit: usize, pub stop_at_first: bool }
pub struct LockstepReport { pub ticks_run: u64, pub divergence: Option<Divergence> }
/// Both steppers start at the same boundary. For each tick t, `writes(t, side)` gives that side's
/// writes for boundary t - 1 (the same on both sides except where a check adds branch writes or
/// a reload, checks.md 8.3, 8.5); each side applies its writes through the live path (2.1), steps
/// and hashes; on a difference both are snapshotted and diffed. Side a is `Expected`.
/// version.schema_differs when the registries differ.
pub fn lockstep(a: &mut dyn Stepper, b: &mut dyn Stepper,
                writes: &mut dyn FnMut(Tick, Side) -> Vec<RecordedWrite>,
                opts: LockstepOptions) -> LockstepReport;
```

Lockstep is the mechanism of the same-process determinism check, the fork checks and reload
equivalence (checks.md 8.2, 8.3, 8.5), and of replaying one recording with old and new scripts
(versions.md 7.3). To name fields where a replay names only sections, the caller re-runs the
recorded side live beside the replayed one in lockstep. Lockstep runs live steppers, so its ticks
belong to one segment.

### 3.4 Verifying a replay

`verify(replay, stepper, options) -> ReplayReport` (its types are in versions.md 7.2, with the modes
that gate it) restores the start or the keyframe at `options.from` and compares its world hash with
the recorded one, then per tick applies the recorded writes through the live path (2.1; a write
refused now stops the run with `replay.write_refused`), steps, hashes and compares. At a `Rebase` it
restores the recorded snapshot, compares its world hash and goes on with the next segment. With
section digests on both sides a difference names sections; a keyframe on the diverging tick names
fields. `End`'s writes and hash come last in each segment.

A `Fault` record stops the run at its tick with the fault's code (`script.out_of_memory`,
`script.stack_overflow`; the outcome `Stopped`), without running that tick: a fault depends on the
platform and the allocation history, so whether the replaying engine faults there too says nothing
about determinism, and a fault is reported as what it is rather than as an unexplained hash
divergence. A `Tainted` record stops the run at its tick with `replay.tainted`: from there the
recording does not reproduce its world (script-host.md 13). `pocket replay --verify` (checks.md 8.4)
is this function.

## 4. Errors

Codes travel in spec-contract's error protocol; the `persist` family is persistence.md 11's,
`version` and `migrate` versions.md 9's.

| Code | When | `detail` |
|---|---|---|
| `replay.format` | Bad magic, format or record tag | `{offset, tag}` |
| `replay.truncated` | Not even `Start` is whole (a shorter cut sets `complete: false`) | `{offset}` |
| `replay.write_refused` | A recorded write is refused on replay by a world-level check | `{segment, tick, write, error}` |
| `replay.bundle_unavailable` | A needed bundle is neither embedded nor substituted, or its compiled form cannot be used without a transpiler | `{hash, reason}` |
| `replay.data_unavailable` | A data file a record names is neither embedded nor in the content store | `{hash, path}` |
| `replay.out_of_range` | A seek, `from` or query tick outside the replay | `{segment, tick, first, last}` |
| `replay.tainted` | `verify` reached a `Tainted` record | `{segment, tick, reason}` |
| `replay.unknown_recording` | A recording id no session or file names (shared/contract/mcp.md 5.7) | `{id, recordings}` |

## 5. Checks

Beside persistence.md 12's P5 (golden replays) and the replay checks of checks.md 8.4, which run
over this specification:

- **R1, segments.** A recorded session that restores a session snapshot, adopts a branch and resets
  verifies across all three `Rebase`s; `seek` to a `TickRef` in each segment reproduces its hash;
  `first_divergence` of two such recordings that differ only in the second segment names a tick of
  segment 1.
- **R2, section digests.** A fixture replay in which sections appear and disappear (a component type
  whose last row is removed, then added again) rebuilds the same running list in two independent
  readers, and a divergence in a reappeared section is named by its key.
- **R3, compiled bundles.** A native recording with `embed_compiled` verifies in the web worker,
  which has no transpiler (checks.md 7.2); with its `compiled` removed it is refused there with
  `replay.bundle_unavailable` and `reason: "needs transpile"`, and verifies natively.
- **R4, data.** A recording with `embed_data` verifies with the project's data files removed;
  without, it stops with `replay.data_unavailable`.
- **R5, faults and taints.** A recording whose run faulted stops `verify` at the fault's tick with
  its code; a recording made while a debugger evaluated inside a tick stops with `replay.tainted` at
  that tick.
- **R6, the live path.** A recorded `act` whose intent `accept` the substituted scripts refuse stops
  `verify` in `Compare` with `replay.write_refused`, and `lockstep` reports `DivergenceKind::Write`;
  a recorded write from a player session replays with no session attached.

## 6. Open choices

1. **Slice 1: `Source` and `Applied` are declared in `pocket-persist`** (`pocket_persist::replay`),
   as threads.md 5.1 and 5.3 give them: `pocket-link`, which threads.md names as their home, depends
   on `pocket-persist` (architecture.md 5), so the recorder could not name them otherwise;
   `pocket-link` re-exports them.
2. **Slice 1: `Stepper::run_config`.** `Verify` refuses another run configuration (versions.md 7.2),
   and only the stepper knows the one it runs, so the trait gives it as canonical JSON.
3. **Slice 1: substitutes are `BundleRecord`s.** `VerifyOptions::substitute` maps a recorded bundle
   hash to persistence's own `BundleRecord`, since `pocket-persist` cannot name the script host's
   `Bundle`; the stepper's `ReplaySource` finds a substitute before the recording's bundle of that
   hash.
4. **Slice 1: the recorder's calls.** `Recorder::start` takes an `Arc<Registry>`, since
   `end_tick(world)` hashes with it and the recorder outlives the call. A restore or a reset
   replaces the world in place, so `rebase` sees only the new one: the runtime calls
   `Recorder::end_segment(world)` before replacing the world (it writes `End` with the last
   boundary's writes and the hash after them) and `rebase` after, which refuses with
   `replay.segment_open {tick}` when the segment was not ended (or faulted). `end_tick` refuses,
   writing nothing, when no segment is open (`replay.segment_closed {tick}`: after `end_segment` or
   a fault, before `rebase`) and when the world's tick does not follow the last one recorded
   (`replay.tick_sequence {expected, found}`: the world was replaced without `end_segment` and
   `rebase`), which the reader would otherwise refuse as a whole file. The recorder stamps the
   bundle it was told the world runs (`start`, `bundle_loaded`, `rebase`) onto a snapshot whose
   world's `SnapshotContext` names none, so every `Start`, `Rebase` and `Keyframe` names its bundle.
   A record that cannot be written, a sink failure or a record that does not encode, inside any
   call, breaks the recording: nothing more is written, so the file stays a readable prefix (a write
   after a partial one would corrupt the framing), and every later call that returns a result
   (`end_tick`, `end_segment`, `rebase`, `formats_changed`, `finish`) returns that problem. The
   failures of `applied`, `refused`, `data_read`, `data`, `bundle_loaded`, `fault` and `tainted`,
   which return nothing, so surface at the next call that returns one; a recording that silently
   lacked a `Data` record would stop `verify` far from its cause.
5. **Slice 1: the reader's extra API.** Beside 2.4: `tick_hashes()` (every `TickRef` with its hash,
   each segment's start point first, the input of `first_divergence`), `digests(at)`,
   `keyframe_at(at)`, `start(segment)`, `end(segment)`, `formats()`, `bundles()`, `records()`, and
   `into_parts`, `from_records` and `to_bytes` to write an edited copy (the localization check's
   copy without one write, checks.md 8.4). The reader checks that each segment's ticks follow one
   another and, with section digests, that the rebuilt list hashes to the tick's hash.
6. **Slice 1: what `verify` does at the edges.** A `from` outside the replay (a segment it does not
   have, or a tick outside that segment's start and last tick) stops it before anything runs with
   `replay.out_of_range {segment, tick, first, last}` and `verified: false`. A `Fault` stops the run
   at the fault's tick, and a `Tainted` record at its tick, both before that tick runs. A write the
   stepper cannot apply because a bundle or a data file is missing stops the run as
   `replay.bundle_unavailable` or `replay.data_unavailable`; any other refusal is
   `replay.write_refused` with the stepper's problem in its detail. With `continue_after_divergence`
   the run stops at the tick where the hashes agree again (`reconverged_at`). A divergence after a
   segment's `End` writes is reported at the segment's last tick. A difference names sections when
   the recording has section digests and fields when it has a keyframe on that tick
   (`keyframe_every: 1` gives both). `version.data_mismatch` is not checked here: comparing a data
   file with its `DataRead` needs the project's current files, which the runtime's stepper reads
   (`pocket-runtime`, next wave). A bundle is looked up as a substitute, then in the recording, then
   by `Stepper::find_bundle(hash)` (versions.md 7.4: the project's current scripts when their hash
   matches, or the content store; none by default), in that order both when a snapshot is restored
   and when the versions are compared: `BundleUse::Available` means the stepper found it, and passes
   `Verify`; `Unavailable` means nothing has it, which `Verify` refuses with `version.mismatch` and
   `Compare` meets as `replay.bundle_unavailable` at the restore.
7. **Slice 1: `seek`** re-simulates with the recorded bundles, checks each tick hash (a difference
   is `replay.diverged`, checks.md 8.4's code) and reports `scripts_changed_since` when a `Bundle`
   record follows the tick's record; loading the session's current bundle afterwards
   (`SeekScripts::Current`) is the runtime's, which owns the script host.
8. **Slice 1: `first_divergence` and `lockstep`.** `first_divergence` pairs the two lists by
   position, both in recording order; where they name different ticks, one run ended its segment
   early (`EndedEarly`). `lockstep` runs both steppers from their current tick and names ticks in
   segment 0; a hash failure or a step error on either side is a `Stopped` divergence.
