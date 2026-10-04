# Versions: engine, scripts, data, schemas, migrations, saves and replays

Status: Draft, slice 0

Charter: 3.3 (replays that name the first divergence), 4.2.6 (hot update; one recording replayed
with old and new scripts; schema changes through version numbers and migration functions), 7.13
(replays record the script bundle hash and engine version; component schemas carry versions and
migration functions; saves hold world data only). Byte formats, hashes and the replay API:
[persistence.md](persistence.md).

## 1. Scope

Besides its writes, a run's result depends on the engine code, the scripts, the run configuration,
the data files it reads and the shapes of the types it stores. This specification fixes how each is
identified, what replays and saves record of them, when two runs are comparable, what happens when a
replay's versions do not match the engine replaying it, and how data stored at an older schema is
migrated. Concepts used by name: bundles, `ComponentSchema`, project components, budgets and hot
update from spec-script ([script-host.md](script-host.md), [hot-update.md](hot-update.md)); the
error protocol and its type `Problem` from spec-contract
([errors.md](../../shared/contract/errors.md)); build scripts, asset identity, the command queue and
the check command from spec-arch ([architecture.md](architecture.md), [threads.md](threads.md),
[checks.md](checks.md)); `Tick`, `EntityId` and the deterministic math library from spec-sim
([simulation.md](simulation.md), [numeric.md](numeric.md)).

The rule in one line: **a replay asserts reproduction, a save asserts continuation.** A replay is
verified only against the same engine, scripts, configuration, data and schemas; a save loads into
any later engine and scripts that can migrate its data.

## 2. What can change, and what records it

| What changes | Identifier | Recorded in |
|---|---|---|
| Engine code, dependencies, toolchain | `EngineVersion.source` (3.1) | Snapshot, replay and save headers |
| The shared contract | `EngineVersion.contract` (3.1) | Same |
| Scripts | Bundle hash (3.2) | Snapshot header; replay `Bundle` records and swap writes |
| Data read by path | Content hash per file (3.3) | Replay `DataRead` records |
| Byte layouts | Format numbers (3.4) | Each file's second field |
| A persisted type's shape | Schema version and fingerprint (3.5) | Every section's framing; format tables (4) |
| A library cache's layout | Cache identity (3.6) | Cache sections' framing; format tables |
| Settings that decide results | Run configuration (3.7) | Replay header |

## 3. Identifiers

### 3.1 Engine version

```rust
// Clone + Eq + Debug + Serialize + Deserialize
pub struct EngineVersion {
    pub semver: String,       // CARGO_PKG_VERSION of the engine's top crate
    pub commit: String,       // `git rev-parse HEAD` at build time, 40 hex digits, or "unknown"
    pub source: ContentHash,  // the source tree the binary was built from (below)
    pub target: String,       // target triple: "x86_64-pc-windows-msvc", "wasm32-unknown-unknown"
    pub profile: String,      // "release", "web", "debug"
    pub contract: String,     // the shared contract's version (charter 6.2): its sync commit
    pub c_compiler: String,   // the compiler that built QuickJS-ng: POCKET_QJS_CC (script-sandbox.md 4.2, P7)
}
impl EngineVersion { pub fn current() -> &'static EngineVersion; }
```

`source` decides whether two binaries run the same code; a commit hash does not, since most
development builds come from a working tree with uncommitted changes, and two builds of one dirty
tree (the native and web builds of one check run) must be recognized as the same code.
`pocket-app`'s build script computes (architecture.md 4.13)

```text
source = BLAKE3-derive_key("Pocket3D 2026-10-03 engine source v1",
           per file in path order: ULEB128(len path) || path || BLAKE3(content))
```

over the files `git ls-files --cached --others --exclude-standard` lists under the paths compiled
into the engine: the crates, `shared/contract/rust/`, the workspace `Cargo.toml` and `Cargo.lock`,
`rust-toolchain.toml`, `.cargo/config.toml`, and the vendored, patched QuickJS-ng under
`third_party/` (`rquickjs-sys-0.14.0/` and `patches/`), which is committed so that the list sees it
(architecture.md 4.13 and 7.5 fix the list). Paths are relative to the repository root with `/`
separators; contents are raw bytes (`.gitattributes` keeps text LF on every machine). Project
scripts and data are not in it.

`target`, `profile` and `c_compiler` are recorded for diagnosis and do not decide comparability:
native and web builds of one `source` must give the same hashes (checks.md 7.2), and features never
change simulation results (architecture.md 7.4). The perf step refuses to time a native build whose
`c_compiler` is not clang-cl (`perf.wrong_compiler`, checks.md 9).

### 3.2 Bundle hash

A bundle is the reachable set of TypeScript modules the script host loads, which "is what runs and
what the bundle hash covers" (script-host.md 8.1). Its hash is

```text
bundle_hash = BLAKE3-derive_key("Pocket3D 2026-10-03 script bundle v1",
                per module in path order:
                  ULEB128(len path) || path || ULEB128(len source) || source)
```

over the TypeScript sources as written, paths as the loader's `ModulePath`s. The transpiled
JavaScript is not hashed: the transpiler is engine code, which `source` covers. Persistence needs
from spec-script a `Bundle` value with its `hash: ContentHash` and `files: Vec<(String, Vec<u8>)>`,
and the `CompiledSet` it was instantiated from (script-host.md 8.1), which a replay embeds beside
the sources with the `EngineVersion.source` that compiled it (replay.md 2.2). A hot update is a Host
write that swaps the bundle of one world at a boundary (threads.md 5.2, 6); the replay records the
write and, when bundles are embedded, the bundle (7.4).

### 3.3 Data files

Assets enter the world by content hash: an import's result is a Host write naming its hash
(threads.md 6; architecture.md 4.2), so the replay's writes already name those bytes. Other files
the game thread reads by path, whose content can reach world state (a scene or prefab a command
names, a terrain source), go through one function,
`pocket_assets::read_data(path) -> (bytes, ContentHash)` (architecture.md 4.2), which returns the
bytes with their BLAKE3 `ContentHash` and reports `(path, hash)` to a running recorder as a
`DataRead` record. With `embed_data` (the default) the recorder also writes the bytes once per hash
as a `Data` record, and an import's result likewise before the write that names it, so a replay on
another machine or in a browser has what it reads (replay.md 2.2 and 2.4). Render-only files are
never read by the simulation (charter 4.4). Data loaded before a recording starts is inside its
start snapshot.

### 3.4 Format numbers

The snapshot, replay and save layouts each carry a `u32` format number, 1 for these specifications.
A change to a layout, to PCE or to a hash definition bumps the numbers it affects. A reader reads
its current formats; older saves are upgraded (8.6); replays and snapshots of another format are
refused with `version.format`.

### 3.5 Schema version and fingerprint

Every persisted resource and component type has a schema version, a `u32` from 1:
`Persisted::VERSION` for engine types, `ComponentSchema.version` for project components
(script-host.md 7.3). It changes only together with a migration (section 8). A fingerprint detects a
shape change made without a bump:

```text
fingerprint = first 16 bytes of BLAKE3-derive_key("Pocket3D 2026-10-03 schema fingerprint v1",
                PCE(resolve(format)))
```

`format` is the type's `serde-reflection` `Format` (persistence.md 6.2), built for a project
component from its declared fields. `resolve` inlines every reference to a named container, keeping
field and variant names and dropping container names; a type that refers to itself fails with
`persist.type`:

```rust
// Clone + Eq + Debug + Serialize + Deserialize
pub enum ResolvedFormat {
    Unit, Bool, I8, I16, I32, I64, I128, U8, U16, U32, U64, U128, F32, F64, Char, Str, Bytes,
    Option(Box<ResolvedFormat>), Seq(Box<ResolvedFormat>),
    Map { key: Box<ResolvedFormat>, value: Box<ResolvedFormat> },
    Tuple(Vec<ResolvedFormat>), TupleArray { content: Box<ResolvedFormat>, size: u64 },
    UnitStruct, NewTypeStruct(Box<ResolvedFormat>), TupleStruct(Vec<ResolvedFormat>),
    Struct(Vec<(String, ResolvedFormat)>),
    Enum(Vec<(u32, String, ResolvedVariant)>),          // ascending variant index
}
pub enum ResolvedVariant {
    Unit, NewType(Box<ResolvedFormat>), Tuple(Vec<ResolvedFormat>),
    Struct(Vec<(String, ResolvedFormat)>),
}
pub struct Fingerprint(pub [u8; 16]);                   // Copy + Eq + Ord + Hash
```

Renaming the Rust struct changes nothing; adding, removing, renaming, reordering or retyping a
field, and adding or reordering enum variants, change the fingerprint. The same resolved formats
drive the decoder that renders old bytes as JSON (8.1) and `diff` (replay.md 3.2).

### 3.6 Cache identity

A cache (persistence.md 8) is opaque, so its layout is named by a string: the library's name and
exact version, the features that change its serialized layout, and a local layout number bumped when
the wrapper changes. For Rapier:
`"rapier3d 0.36.0 parry3d 0.31.1 dim3 f32 serde-serialize enhanced-determinism layout 1"`. Its
section's version is 0 and its fingerprint is the first 16 bytes of the BLAKE3 hash of the string.
The library is pinned with `=` (architecture.md 6), and a test compares the string with the versions
in `Cargo.lock`.

### 3.7 Run configuration

Some settings decide results without being world state: the script limits `steps_per_system`,
`steps_per_tick`, `steps_per_call` and `max_call_depth` decide which calls fail (script-sandbox.md
4.1, "run configuration that [persist] records with a replay"), and the halt policy decides what a
failure does (script-sandbox.md 5.4; shared/contract/time.md, Halts). Their owners list them; the
replay header carries them as `run_config`, one JSON object in the canonical text of replay.md 2.1
(keys sorted). A setting that never changes results stays out: the log limits, since console
formatting runs no user code and spends no steps (script-host.md 5.7), and the presenter's frame
rate.

## 4. Format tables

Files that leave the process carry the formats of their sections, so an engine whose types have
since changed can still decode them:

```rust
pub struct FormatTable { pub entries: Vec<FormatEntry> }   // in section order
pub struct FormatEntry {
    pub section: SectionKey,
    pub version: u32,
    pub fingerprint: Fingerprint,
    pub format: Option<ResolvedFormat>,   // None for caches
    pub identity: Option<String>,         // caches only
}
```

A save holds one table; a replay holds one after its start and another whenever a hot update
registers a project component (replay.md 2.2, `Formats`).

## 5. Schema lock

Version numbers rest on remembering to bump them, so a committed lock file holds the last accepted
schema of every type and a check compares the registry with it (charter 3.7: checks, not discipline;
`buf breaking` checks protobuf schemas against their last release the same way). The engine's lock
is `schema.lock.json` at the repository root, regenerated by `cargo xtask gen --locks`
(architecture.md 4.15); each project has its own beside its `project.toml` for its components,
regenerated by `pocket gen --locks <project>`. It is generated, in section order, one entry per
type: kind, name, version, fingerprint, resolved format, or for a cache its identity. Keeping
formats in the lock lets the check name the field that changed.

| Registry against lock | Result |
|---|---|
| Same version and fingerprint | Pass |
| Fingerprint differs, version equal | `version.unbumped`, e.g. "Boat changed shape (field `/reef` added) but its version is still 2: set it to 3 and add a migration from 2" |
| Version one higher, step from the locked version present, examples pass | `version.lock_stale` until the lock is regenerated by `cargo xtask gen --locks` or `pocket gen --locks` (which refuse while a step is missing) |
| Version higher, a step missing | `migrate.missing_step` naming each missing `from` |
| Version lower | `version.downgrade` |
| A type the lock lacks | `version.lock_stale` (new types enter at version 1) |
| A locked type the registry lacks | `migrate.missing_removal` unless retired (8.4) |

Agents meet this check in the apply command for project components (script-host.md): an agent that
adds a field without bumping the version gets `version.unbumped` naming the field and the step to
add.

## 6. Saves

### 6.1 What a save holds

A save holds world data only (charter 7.13): the persisted sections of a snapshot (persistence.md
4), the format table and a label. No scripts, no script VM state, no renderer state, no observers'
sessions, no replay history. Master's saves wrote what each script context returned from `onSave`
beside the scene (master `docs/design/world-model.md`, Saves), and its planner needed `onSave` to
keep tweens in phase for a branch to play out exactly (master `docs/design/environment.md`,
Learning); here all of that is component state already (charter 3.2). Caches are included, so a save
loaded into the same engine continues bit for bit.

```text
save = "P3DSAVE" 0x00  format:u32-LE (1)
       ULEB128 length, PCE(SaveHeader)
       ULEB128 length, PCE(FormatTable)
       snapshot bytes (persistence.md 4.2, whole, with their header and world hash)
```

```rust
pub struct SaveHeader { pub label: String, pub meta: BTreeMap<String, String> }
pub fn write_save(world: &World, reg: &Registry, label: &str, meta: BTreeMap<String, String>)
    -> Result<Vec<u8>, Problem>;
```

Where saves are stored (a user data directory natively, browser storage on the web) is spec-arch's.

### 6.2 Loading a save

```rust
pub struct LoadReport {
    pub migrated: Vec<(SectionKey, u32, u32)>,  // section, from, to
    pub renamed: Vec<(String, String)>,
    pub dropped: Vec<SectionKey>,               // retired types (8.4)
    pub caches_rebuilt: Vec<SectionKey>,        // identity differed (6.3)
    pub engine_differs: bool,                   // informational
    pub bundle_differs: bool,                   // informational
}
/// A snapshot in the current versions; restore it (persistence.md 6.3), then call
/// `PersistedCache::rebuild` for each entry of `caches_rebuilt`.
pub fn load_save(bytes: &[u8], reg: &Registry, migrations: &Migrations,
                 project: &mut dyn ProjectMigrator) -> Result<(Snapshot, LoadReport), Problem>;
/// Runs a project component's TypeScript steps (8.3). Implemented by pocket-runtime with the
/// script host, so pocket-persist does not depend on it (architecture.md 4.3, 9 item 2).
pub trait ProjectMigrator {
    fn migrate(&mut self, component: &str, from: u32, value: serde_json::Value)
        -> Result<serde_json::Value, Problem>;
}
```

The steps, all before any world is touched:

1. Check the magic and format; upgrade an older format (8.6); refuse a newer one (`version.format`).
2. Apply registered renames (8.4) to section names.
3. Per section: same version and fingerprint as the registry, keep the bytes. Same version, other
   fingerprint: `version.fingerprint_mismatch` (the lock check exists to make this impossible).
   Older: decode the rows to JSON with the save's format (8.1), run the steps up to the current
   version, check the shape, encode with the current type. Newer: `version.newer`. Retired as
   removed: drop and list. Unknown and not retired: `version.unknown_section`. A cache with another
   identity: drop and list in `caches_rebuilt`.
4. Assemble a snapshot in canonical section order with the save's tick and the current engine
   version and bundle in its header.

A migrated world hashes differently from the saved one, as it should; the report says what changed.
Engine and bundle differences never refuse a load.

### 6.3 Caches across versions

A cache whose identity matches is restored as written, and the world continues exactly. One whose
identity differs is rebuilt from the component sections by `PersistedCache::rebuild`, which every
cache MUST implement (persistence.md 8): physics from the rigid body, collider and joint components,
`Transform` and velocities. A rebuild is inexact (contacts restart without warm-start impulses,
sleeping bodies may wake), so a physics library upgrade changes how a loaded save continues for a
moment, never whether it loads.

## 7. Replays and versions

### 7.1 What is compared

```rust
pub enum Match { Same, Differs { recorded: String, current: String } }
pub enum BundleUse { Same, Substituted { with: ContentHash }, Available, Unavailable }
pub struct VersionComparison {
    pub format: Match,
    pub engine: Match,                                // `source` only
    pub contract: Match,
    pub run_config: Match,
    pub bundles: Vec<(ContentHash, BundleUse)>,       // every bundle the replay loads
    pub schemas: Vec<(SectionKey, Match)>,            // version and fingerprint
    pub caches: Vec<(SectionKey, Match)>,             // identity
    pub data: Vec<(String, Match)>,                   // filled as DataRead records are met
}
```

### 7.2 Modes and `verify`

```rust
pub enum ReplayMode { Verify, Compare, Rerun }
pub struct VerifyOptions {
    pub mode: ReplayMode,
    pub from: Option<TickRef>,                         // start at a keyframe (replay.md 2.4)
    pub substitute: BTreeMap<ContentHash, Bundle>,     // run other scripts (7.3)
    pub continue_after_divergence: bool,               // to find reconverged_at
}
pub enum ReplayOutcome { Identical, Diverged(Divergence), Stopped(TickRef, Problem) }
pub struct ReplayReport { pub ticks_run: u64, pub outcome: ReplayOutcome,
                          pub versions: VersionComparison, pub verified: bool }
pub fn verify(replay: &Replay, s: &mut dyn Stepper, opts: VerifyOptions) -> ReplayReport;
```

The caller chooses the mode; nothing switches modes silently.

| Mode | Requires equal | May differ (reported) | Compares | Use |
|---|---|---|---|---|
| `Verify` | Format, engine `source`, contract, run configuration, every bundle (none substituted), schemas, cache identities, data as met | `target`, `profile`, `commit` | Every tick hash | The replay check (checks.md 8.4, 7.2); a recording checked on another machine or in a browser |
| `Compare` | Format, schemas, cache identities | Engine, contract, run configuration, bundles (substitutes allowed), data | Every tick hash; the first divergence comes with the comparison | Golden replays across engine changes (persistence.md P5); new scripts on an old recording |
| `Rerun` | Format | Everything else; older schemas migrate and caches rebuild as for a save (6.2) | Nothing (`verified: false`) | Watching an old match in a newer engine |

When `Verify`'s requirements fail, nothing runs: the outcome is `version.mismatch` with the whole
`VersionComparison` as detail, so every difference is seen at once. In `Verify`, a data file whose
hash differs from its `DataRead` record stops the run at that tick with `version.data_mismatch`.
`Compare` refuses differing schemas or cache identities with `version.schema_differs`, because the
hash covers each section's version and every tick would differ by construction (persistence.md 5.2);
its detail suggests `Rerun`, or lockstep with matching bundles. In every mode a recorded write is
applied through the live application path, world-level validation and an intent's `accept` included,
with only the session-level checks skipped (replay.md 2.1); a write the current engine or scripts
refuse at world level stops the run with `replay.write_refused` at its tick.

### 7.3 Old and new scripts on one recording

Charter 4.2.6 asks to replay one recording with old and new scripts and find the first tick where
they diverge. Both ways run on the current engine:

- `verify` in `Compare` mode with `substitute: {old_hash: new_bundle}`: the recorded writes run
  under the new scripts, and the first tick whose hash departs from the recorded one is named, with
  the differing sections when the recording has section digests.
- `lockstep` (replay.md 3.3) with two steppers restored from the recording's start or a keyframe,
  one with the recorded bundle and one with the new, fed the recorded writes: the tick, the sections
  and every differing field with both values.

Both require the two bundles to declare the same project component versions
(`version.schema_differs` otherwise). The use: an agent changes a script, replays the match that
went wrong, and learns at which tick its change first altered the world and in which fields.

### 7.4 Embedded bundles

By default the recorder embeds every bundle a run loads, once, before its first use, with the
compiled modules beside the TypeScript sources and the bytes of every data file the run reads, so a
replay is self-contained: another machine or a browser given only the file can re-run it (charter
5.1, the browser-local form for watching replays). The shipped web build has no transpiler, so it
instantiates the embedded JavaScript, which it may do when the recording engine's
`EngineVersion.source` equals its own; any other case needs `transpile` and is refused without it
with `replay.bundle_unavailable {hash, reason: "needs transpile"}` (replay.md 2.4). With
`embed_bundles: false` the stepper must find each bundle by hash (the project's current scripts when
their hash matches) or be given a substitute, else `replay.bundle_unavailable`; with
`embed_data: false` it finds data in the content store by hash, else `replay.data_unavailable`. A
save never contains scripts.

### 7.5 Seek, restore and fork across a hot update

- The bundle is a binding the runtime keeps per world. A fork inherits its source's bundle at the
  fork; a later swap on the source does not reach it, and a swap addressed to the fork (`world` in
  `scripts.apply` and in the MCP `apply`, hot-update.md 4.1) does not reach the source. Each
  branch's recording opens with the `Bundle` record of the bundle it inherited (replay.md 2.5).
- `seek` to tick `t` inside a session's history (replay.md 2.4) re-simulates from a keyframe with
  the recorded bundles, so boundary `t` is reproduced exactly; then, with `SeekScripts::Current`
  (the default), it loads the session's current bundle at that boundary, as a hot update would, so
  an agent can try its fix on the state that failed. `SeekScripts::Recorded` keeps the recorded
  bundle. The report says whether the scripts changed since `t`.
- When the current bundle declares a newer version of a project component than the world at `t`
  holds, that load migrates the world as a hot update does (8.5).
- `restore` of an in-memory snapshot changes the world only, and the scripts stay current. A
  snapshot taken before a hot update that changed a project component's version is migrated first,
  with the formats the session's registry keeps for every version it has registered (8.5); a
  snapshot kept on disk is a save and loads through 6.2.

## 8. Migrations

### 8.1 Contract

A migration step takes one section type from version `n` to `n + 1`.

- **One version at a time.** A save at version 1 loaded by an engine at version 4 runs 1 to 2, 2 to
  3, 3 to 4. Steps form an append-only ordered list, as database migration tools keep them (Flyway,
  Diesel, Rails); a step runs when its section is at its `from` version, in list order.
- **Pure and deterministic.** A step computes its output from its input (and, for a world step, the
  other sections) alone: no clock, no randomness, no I/O, floating point limited to `+ - * /`,
  `sqrt` and spec-sim's deterministic math library as in simulation code (charter 4.2.4). Migrations
  run inside replays (`Rerun`, and hot updates replayed as writes), so they are simulation code.
- **JSON as the medium.** The decoder renders a section written at version `n` into
  `serde_json::Value`s with that version's `ResolvedFormat`: a struct is an object keyed by field
  name; an enum is externally tagged (`"Calm"`, `{"Gust": {...}}`); `Option` is `null` or the value;
  sequences and tuples are arrays; a map with string keys is an object, any other an array of
  `[key, value]` pairs; bytes are an array of numbers; `u128` and `i128` are decimal strings; floats
  are numbers with their bits, `-0.0` included. A NaN or infinity fails with `migrate.nonfinite`, as
  JSON cannot carry it.
- **A conversion directed by the schema.** A step's output is converted by the current
  `ResolvedFormat`, never read back by `serde` alone: an integer field accepts any integral number
  (`5.0` as well as `5`) within its range, by numeric.md 8; a float field takes the number's double
  with its bits, `-0` kept, and an integer JSON number as its exact double; NaN and infinities fail
  with `migrate.nonfinite`. Missing or extra fields, wrong JSON types and integers out of range fail
  with `migrate.shape` naming the JSON Pointer path. The converted value is then encoded with PCE
  (or checked against the project's `ComponentSchema`). So a step's result has one encoding whatever
  form the step returned a number in.
- **Examples are required.** Every step carries at least one `(old, new)` pair; the check runs each
  and compares the expected and actual values after this conversion, as PCE bytes, not as JSON text.
  A step without examples, or whose example fails, is `migrate.untested`.
- **Atomic.** A failing step fails the whole load or hot update, which then changes nothing.

### 8.2 Engine migrations in Rust

```rust
pub struct MigrationStep {
    pub kind: SectionKind,
    pub section: &'static str,                              // the name at version `from`
    pub from: u32,                                          // produces from + 1
    pub apply: Apply,
    pub examples: &'static [(&'static str, &'static str)],  // (old JSON, new JSON)
}
pub enum Apply {
    /// Each component row or the resource value.
    Value(fn(&MigrationCtx, serde_json::Value) -> Result<serde_json::Value, MigrationError>),
    /// The whole world, for steps that split, merge or move data between sections.
    World(fn(&mut DynWorld) -> Result<(), MigrationError>),
}
pub struct MigrationCtx { pub entity: Option<EntityId>, pub tick: Tick }
/// Sections as JSON: component rows by EntityId, resources as one value, caches as bytes.
pub struct DynWorld { /* BTreeMap<SectionKey, DynSection> */ }
pub struct Migrations { /* the engine's MIGRATIONS and RETIRED lists (8.4) */ }
```

An example: a sail's 0-to-1 `sail` setting becomes the length of sheet let out, full sail being 4 m.

```rust
MigrationStep {
    kind: SectionKind::Component, section: "Boat", from: 1,
    apply: Apply::Value(|_, mut v| {
        let sail = v["sail"].as_f64().ok_or_else(|| MigrationError::field("/sail"))?;
        let o = v.as_object_mut().ok_or_else(|| MigrationError::field(""))?;
        o.remove("sail");
        o.insert("sheet".into(), serde_json::json!(sail * 4.0));
        Ok(v)
    }),
    examples: &[(r#"{"sail":0.5,"throttle":0.0}"#, r#"{"sheet":2.0,"throttle":0.0}"#)],
}
```

A `MigrationError` becomes `migrate.failed` with `{section, entity, from, path, message}`.

### 8.3 Project migrations in TypeScript

script-host.md 7.3 lets a project component declare migration functions under a `migrate` key, run
by the host "like systems: budgeted, transactional, a value in and a value out, without world
access". The contract:

- `migrate[n]` receives the version-`n` value as a plain object in the JSON form of 8.1, handed into
  JavaScript directly rather than through JSON text, so `-0` stays `-0`, and returns the
  version-`n + 1` value, converted back by script-host.md 5.8 and then by the current
  `ResolvedFormat` (8.1), under the frozen globals, the deterministic `Math` and the step budget of
  any script call (script-sandbox.md 2 and 4.4).
- An exception or an overrun fails with `migrate.failed`, its detail carrying the source-mapped
  TypeScript file and line (script-sandbox.md 5.2).
- Each step declares examples beside it, run by the check as for engine steps.
- Steps take one value at a time; a project step that needs other components is open choice 3.

An illustration (the syntax is spec-script's):

```ts
export const Plot = component("Plot", {
  version: 2,
  doc: "A field plot: growth stage, water and what is sown.",
  fields: { stage: field.i32(0, "growth stage"), water: field.f64(0, "litres"),
            crop: field.str("wheat", "what is sown") },
  migrate: {
    1: { run: (v) => ({ ...v, crop: "wheat" }),
         examples: [[{ stage: 1, water: 0.5 }, { stage: 1, water: 0.5, crop: "wheat" }]] },
  },
});
```

`ProjectMigrator` (6.2) is the host side, implemented by `pocket-runtime` over the current bundle's
declarations.

### 8.4 Removed and renamed types

```rust
pub enum Retired {
    Removed { kind: SectionKind, section: &'static str, last_version: u32 },
    Renamed { kind: SectionKind, from: &'static str, to: &'static str, at_version: u32 },
}
```

A section of a removed type is dropped from a loaded save and listed in `dropped`. A renamed one is
read under its new name at the version it had, then migrated onward. Retirements are permanent, so
older saves keep loading. A project declares its retirements with its components (spec-script's
syntax).

### 8.5 The live world during a hot update

Engine types change only between builds. Project components can change during a run, when a hot
update loads a bundle that declares a newer version (spec-script owns when and how bundles swap). At
the boundary where the swap applies, before any tick runs the new scripts, the world is migrated:
`snapshot`, then `load_save`'s section migration (6.2) with the new bundle's registry and steps,
then `restore`. If a step fails, the swap fails as a whole: the old scripts and the unmigrated world
stay and the error is returned (charter 4.2.6). The recorder records the swap write and the new
`Formats`; replaying the swap re-runs the same deterministic migration. A hot update tried first in
a fork (charter 4.2.6) migrates only the fork.

### 8.6 Format upgrades

When a format number changes, a function from format `n` to `n + 1` keeps saves loadable:

```rust
pub static SAVE_FORMAT_UPGRADES: &[fn(&[u8]) -> Result<Vec<u8>, Problem>] = &[ /* 1 -> 2 */ ];
```

Each has a committed fixture saved in the old format, whose upgrade and load the check runs. Replays
are not upgraded; they are tied to the format that wrote them (open choice 5).

## 9. Errors

Codes travel in spec-contract's error protocol.

| Code | When | `detail` |
|---|---|---|
| `version.format` | A file's format is newer than the engine's, or an old replay or snapshot format | `{kind, found, supported}` |
| `version.mismatch` | `Verify` mode with versions that differ | The `VersionComparison` |
| `version.schema_differs` | `Compare` or lockstep across different schemas or cache identities | `{sections, suggestion}` |
| `version.data_mismatch` | `Verify` meets a data file whose hash differs | `{tick, path, recorded, current}` |
| `version.newer` | A save section at a newer version than the engine knows | `{section, found, engine}` |
| `version.fingerprint_mismatch` | Same version, different shape | `{section, version}` |
| `version.unknown_section` | A save section neither registered nor retired | `{section}` |
| `version.unbumped` | Lock check: the shape changed, the version did not | `{section, version, changes}` |
| `version.lock_stale` | Lock check: the lock needs regenerating | `{sections}` |
| `version.downgrade` | Lock check: a version went down | `{section, locked, current}` |
| `migrate.missing_step` | No step from a version to the next | `{section, from}` |
| `migrate.missing_removal` | A locked type vanished without a retirement | `{section}` |
| `migrate.failed` | A step returned an error or threw | `{section, entity, from, path, message, source}` |
| `migrate.shape` | A migrated value does not fit the current type | `{section, entity, path, expected, found}` |
| `migrate.nonfinite` | An old value holds NaN or an infinity | `{section, entity, path}` |
| `migrate.untested` | A step lacks examples, or an example gives another result | `{section, from, expected, actual}` |

`changes` in `version.unbumped` lists the field paths added, removed or retyped, from the locked and
current resolved formats.

## 10. Checks

Beside persistence.md's P1 to P7, run in the check command (charter 3.7; checks.md 4):

- **V1, schema lock.** Every rule of section 5, for the engine and each sample project, with a test
  that sets up each condition.
- **V2, migration examples.** Every engine and project step's examples give their expected values.
- **V3, save round trip.** `write_save`, `load_save` and `restore` in the same engine give a
  snapshot equal to the original byte for byte, and the restored world continues in lockstep with
  the original.
- **V4, old saves.** A save of the sailing scene is committed whenever a type it uses changes
  version; each loads, migrates and runs 600 ticks without error.
- **V5, the version gate.** A recording replayed with another bundle is refused by `Verify` with the
  bundle in the detail, runs in `Compare` with it flagged, runs in `Rerun`; one whose schema differs
  is refused by `Compare` and runs in `Rerun`; a changed data file stops `Verify` at the tick it is
  read; a changed `run_config` is refused by `Verify`.
- **V6, old and new scripts.** A recording of the sailing bot and a bundle that changes the sail's
  lift: `Compare` with the substitute names the first tick after the sail first draws and the
  sections (`Boat`, `Transform`, the physics cache); lockstep names the fields.
- **V7, hot update with migration.** A recorded run that hot-updates a project component from
  version 1 to 2 replays in `Verify` with identical tick hashes before and after the swap.
- **V8, cache rebuild.** A save whose physics identity is altered loads with `caches_rebuilt` naming
  the physics section and runs 600 ticks.
- **V9, one source, two targets.** The native and web builds of one working tree report the same
  `EngineVersion.source` (checks.md 7.2, `tests`).
- **V10, migration conversion.** A Rust step and a TypeScript step that return `-0`, `5.0` for a
  `u32` field and an integer `0` for an `f64` field give the same PCE bytes as the expected values;
  a step returning NaN fails with `migrate.nonfinite`.

## 11. Open choices

1. **Lock file locations.** Settled (5): `schema.lock.json` at the repository root for engine types
   and in each project for its components (architecture.md 3).
2. **`target` and `profile` in `Verify`.** Recommended: not compared, since checks.md 7.2 holds
   native and web to the same bits. The physics spike found one platform difference, Rapier's own
   calls to the platform's math natively, and it can be removed (numeric.md 5), so the
   recommendation stands; were one found that cannot, `Verify` would compare `target` too and
   cross-target replays would become `Compare` runs.
3. **World-level project migrations.** TypeScript steps that read or write several components. Not
   in slice 1; until then a split is per-value steps plus a retirement.
4. **How long saves stay loadable.** Recommended: always, since steps are append-only and carry
   their examples; revisit if the list outgrows what the checks cover.
5. **Replays across formats.** Refused (8.6). A converter is worth writing only if long-lived replay
   archives appear, such as benchmark records kept across format changes.
6. **Slice 1: the engine version.** `EngineVersion::current()` is the version the top crate installs
   once (`EngineVersion::install`, from its build script's source hash, architecture.md 4.13); until
   `pocket-app` exists every build reports `EngineVersion::unbuilt()`: `source` the derivation over
   no files, `commit` "unknown", `target` the architecture and operating system, `profile` "debug"
   or "release", `contract` "unsynced". `version::source_hash` and `version::bundle_hash` compute
   3.1's and 3.2's hashes from `(path, content)` pairs.
7. **Slice 1: `load_save` takes the world it will restore into**, whose component registry gives the
   project components' current schemas (the registry alone does not know them), and reports the
   caches to rebuild; `save::rebuild_caches` runs their `rebuild` after the restore. A section kept
   under a new name (8.4) gets its digest again, since the digest covers the name. Engine steps run
   in list order, each when its section is at its `from` version; project components then go through
   `ProjectMigrator` one version at a time (`NoProjectMigrator` refuses with
   `migrate.missing_step`).
8. **Slice 1: the conversion of a step's result (8.1)** is `format::json::from_json`, directed by
   the resolved format; `serde_json` turns a NaN or an infinity into `null`, so a `null` where a
   float belongs is `migrate.nonfinite`. A `u128` or `i128` is a decimal string in JSON and is also
   accepted as a number. Recursive formats (persistence.md 14, choice 17) convert through their back
   references.
9. **Slice 1: examples (V2).** `Migrations::check_examples` compares a value step's expected and
   actual values as PCE bytes when the step produces the current version, whose format the registry
   has, and as JSON otherwise, since no format of an intermediate version is kept until the schema
   lock holds one. A world step's examples are required but not run.
10. **Slice 1: the schema lock** is `lock::SchemaLock` (format 1, the sections in section order with
    version, fingerprint and resolved format or cache identity, written as pretty JSON) and
    `lock::check`, which implements every row of section 5 (`version.unbumped` lists the fields
    added, removed, retyped or reordered). Generating and committing `schema.lock.json` waits for a
    binary that links the engine's crates: `xtask` links none (architecture.md 4.15).
11. **Slice 1: `Rerun` migrates.** Each snapshot `verify` restores in `Rerun` (a segment's `Start`
    or `Rebase`, or the keyframe at `from`) goes first through the per-section steps of a load
    (6.2), `save::migrate_snapshot` (which `load_save` calls after reading a save's header and
    format table), with the recording's formats: for each section, the entry of a `Formats` record
    with its key, version and fingerprint. The migrated snapshot is restored, then the caches whose
    identity differs are rebuilt (6.3). Engine steps come from `VerifyOptions::migrations`; a
    project component's steps from `Stepper::migrate_project`, which the runtime implements with its
    script host and which refuses with `migrate.missing_step` by default; the rebuild runs on
    `Stepper::world_mut`. `Verify` and `Compare` restore recorded snapshots unchanged, since they
    require equal schemas. `tests/replay_edges.rs` reruns a recording made with `Pos` at version 1
    in an engine with version 2 and another cache identity, from the start and from a keyframe,
    after `Compare` refused it (V5).

## 12. Sources

- Master: `docs/design/world-model.md` (Saves: the scene plus `onSave` script data),
  `docs/design/environment.md` (Learning: save slots as branch points need `onSave` and tween
  snapshots; Limits: saves without the contact cache branch inexactly), `docs/development.md`
  (Tests: golden hashes change only deliberately, with the reason in the commit).
- PocketEngine charter (`origin/rebuild:docs/charter.md`), 3.3 and 8.2: ids changed after a load,
  the RNG and the contact cache were not saved.
- Practice: append-only ordered migrations (Flyway, Diesel, Rails); schemas checked against a
  committed baseline (`buf breaking`); RFC 6901 for paths.
