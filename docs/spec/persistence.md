# Persistence: canonical encoding, world hash, snapshots, forks and replays

Status: Draft, slice 0

Charter: 3.3 (determinism, fork and replay), 3.7 (the checks these operations serve), 3.10 (fork
time as a budget), 4.1 (`serde`), 5.1 (the canonical data snapshots carry), 7.5 (canonical
serialization), 7.6 (world hash). Versions, migrations and saves: [versions.md](versions.md).

## 1. Scope and related specifications

This specification fixes the byte format in which the world is written (the Pocket Canonical
Encoding, PCE), the world hash defined over it, snapshot, restore and in-memory fork, the replay
file, and how two runs are compared to name the first tick where they diverge and the component that
diverged, with the Rust API of each and the checks that prove them. Concepts owned elsewhere are
used by name:

| Owner | Concepts used here | File |
|---|---|---|
| spec-sim | `Tick` (ticks completed), boundaries and boundary writes, `SimClock`, `EntityId`, `EntityAllocator`, `EntityIndex`, iteration order, events, `EventCounter`, `EventInbox` and the tick's event record, the world seed, numeric rules | [simulation.md](simulation.md), [rng.md](rng.md), [numeric.md](numeric.md) |
| spec-script | `ComponentSchema` and project components, bundles and the reachable module set, run configuration (budgets), hot update | [script-host.md](script-host.md), [script-sandbox.md](script-sandbox.md), [hot-update.md](hot-update.md) |
| spec-contract | the error protocol `{code, message, detail}` and its type `Problem`, seats (`SeatId`) | [errors.md](../../shared/contract/errors.md), [README.md](../../shared/contract/README.md) |
| spec-arch | crates and per-type hooks in `pocket-sim`, the command queue and `Applied`, `WorldSnapshot` publication, the check command's procedures, budgets, the web build | [architecture.md](architecture.md), [threads.md](threads.md), [checks.md](checks.md), [budgets.md](budgets.md) |
| spec-mcp | MCP tools for fork, restore and replay queries, built on section 6 and [replay.md](replay.md) | [mcp.md](../../shared/contract/mcp.md) |

The code is `pocket-persist` (architecture.md 4.3); the traits of 6.2 are declared in `pocket-sim`
(architecture.md 4.1), so physics and scripts register state without depending on it. Its external
crates, pins and features are in architecture.md 6.

## 2. What is world state

All game state is in the `bevy_ecs` world (charter 3.2; simulation.md 4.1, invariant 4), so a
snapshot is a function of the world alone. Every type that can appear in it is in exactly one class:

| Class | Meaning | Snapshot and hash |
|---|---|---|
| Entities | The live `EntityId`s and the `EntityAllocator` | One section |
| Resource | A persisted singleton: `SimClock`, the world seed (rng.md 5.4), `EventCounter` and `EventInbox` (simulation.md 5.1), the time model's state, wind and sea state that evolves | One section per type |
| Component | A persisted component type, engine or project | One section per type with at least one row |
| Cache | A library's state across ticks that changes results: the physics solver's bodies, contact manifolds with warm-start impulses, islands, sleep state | One section per cache, opaque bytes |
| Derived | Recomputed from persisted state with identical results at any boundary: `EntityIndex`, spatial indexes, render data | No; rebuilt after restore |
| Ignored | `bevy_ecs` internals that carry no game state, each with a stated reason | No |

Rules:

- A type in no class makes `snapshot` fail with `persist.unclassified`. Skipping it silently is how
  master lost state: its saves left out the 3D solver's contact cache, so a branch from a save was
  "close, not identical" (master `docs/design/environment.md`, Limits).
- Anything that persists across ticks and influences a later tick is Resource, Component or Cache. A
  Derived type must be rebuildable from persisted state alone; the restore-continuation check (P3)
  proves it.
- Render state, the script VM's heap and observers' sessions are never world state (charter 4.4,
  4.2.5; master kept water rings out of its hash, `docs/design/water.md`). Saves hold world data
  only (versions.md 6).
- Events are world state while the next tick reads them: the `EventInbox` and `EventCounter`
  resources (simulation.md 5.2). The outbox is empty at every boundary, and history beyond one tick
  is kept by its consumers, outside the world.
- `bevy_ecs` change detection, hooks and observers MUST NOT drive simulation logic (simulation.md
  4.5 owns the ban): restore inserts every component afresh. P3 catches a breach. Publication may
  use change detection to reuse unchanged sections (threads.md 4.3); it changes no state.

## 3. The canonical encoding (PCE v1)

PCE is a binary `serde` data format: a value's bytes depend only on the value and its type, never on
memory layout, allocation history, pointers or platform. It follows Binary Canonical Serialization
(BCS, the `serde` format Diem and its successors hash) in fixed-width little-endian integers and
ULEB128 lengths, adds IEEE-754 floats, and differs in map order (3.3).

### 3.1 Values

| `serde` data model | Bytes |
|---|---|
| `bool` | `0x00` or `0x01` |
| integers, 8 to 128 bits | Fixed width, little-endian, two's complement |
| `f32`, `f64` | `to_bits()`, 4 or 8 bytes little-endian; no canonicalization: `-0.0` and `+0.0` differ, NaN payloads are kept |
| `char` | The scalar value as `u32` |
| `str`, `bytes` | ULEB128 byte length, then the bytes (UTF-8 for `str`) |
| `none` / `some(v)` | `0x00` / `0x01` then `v` |
| `unit`, unit struct | Nothing |
| enum variants | ULEB128 variant index, then the fields if any |
| newtype struct | The inner value |
| tuple, tuple struct, struct | The fields in declaration order, no names, no count |
| `seq` | ULEB128 element count, then the elements |
| `map` | ULEB128 entry count, then key and value per entry, in the map's iteration order |

`is_human_readable()` is `false`. `usize` and `isize` reach the format as 64-bit values (`serde`
serializes them so), and a value above `u32::MAX` fails to decode on `wasm32`, so persisted types
use explicit widths. Floats are bit-exact because the hash exists to detect runs that will diverge,
and the sign of a zero or a NaN payload can change a later result (`atan2(-0.0, -1.0)` is `-π`). PCE
itself encodes every float bit for bit, non-finite values included. The finiteness check (numeric.md 7)
belongs to the typed encoders of Component and Resource sections, which refuse a NaN or an infinity
with `persist.encode`, and to `PlainData` (3.4); it never applies to Cache sections, which hold a
library's state as the library keeps it: Rapier's default joint motor has
`max_force: Real::INFINITY` (rapier3d 0.36.0, `src/dynamics/joint/generic_joint.rs`), so a world
with a joint could not be snapshotted otherwise. Rapier under `enhanced-determinism` forces `-0.0`
in stored solver state to `+0.0` itself, because platforms disagree on the sign of a zero that `min`
or `max` picks (rapier3d 0.36.0 `src/utils/mod.rs`, `canonicalize_zero`); PCE records what it is
given.

### 3.2 Lengths

Lengths, counts and variant indices are ULEB128 (as in DWARF and WebAssembly) holding a `u64`,
minimal: the decoder rejects a trailing `0x00` byte and more than 10 bytes. `serialize_seq` or
`serialize_map` without a length fails with `persist.encode`.

### 3.3 Containers and order

The encoder writes a map in its iteration order and never sorts. A persisted type therefore uses
only containers whose order is a function of their contents or operation history and survives a
round trip: `Vec`, arrays, `BTreeMap`, `BTreeSet`, `IndexMap`, `IndexSet`.
`std::collections::HashMap`, `HashSet` and `hashbrown` maps are forbidden in persisted types
(simulation.md 8.4 bans their iteration in simulation code): their order depends on hasher keys and
capacity history, so equal worlds would write different bytes, and a map rebuilt by restore would
iterate differently.

Sorting entries by encoded key, as BCS and deterministic CBOR (RFC 8949, 4.2.1) do, was rejected: an
`IndexMap` restored from sorted entries iterates in sorted order instead of its insertion order, so
code iterating it would act differently after a restore. Rapier under `enhanced-determinism` keeps
its maps as `IndexMap` with a fixed `FxHasher32` (parry3d 0.31.1 `src/utils/hashmap.rs`) and writes
the one whose order must not matter sorted by key itself (rapier3d 0.36.0
`src/dynamics/island_manager/persistent.rs`, `serialize_joint_link_locs`); keeping iteration order
is correct for both.

### 3.4 Plain data

Event data (simulation.md 5.1) and the plain data scripts pass (script-host.md 5.1, `Data`) have one
canonical form:

```rust
pub enum PlainData {   // variant indices 0..=5 in this order
    Null, Bool(bool), Number(f64), String(String), Array(Vec<PlainData>),
    Object(Vec<(String, PlainData)>),   // keys ascending by UTF-8 bytes, no duplicates
}
```

`Number` is finite (a NaN or infinity fails with `persist.encode`, as scripts refuse them,
script-host.md 6). `Object` keys are sorted by their UTF-8 bytes, the order `BTreeMap<String, _>`
and `serde_json`'s default map use, so an object's bytes do not depend on the order its keys were
set in.

### 3.5 What a persisted type may contain

A Resource or Component type MUST derive `Serialize` and `Deserialize` with the default externally
tagged representation; MUST NOT use `#[serde(flatten)]`, `untagged`, `tag`, `content`, `skip`,
`skip_serializing_if` or a `Deserialize` that calls `deserialize_any`, none of which a
non-self-describing format reads back; refers to entities by `EntityId`, never `bevy_ecs::Entity`
(charter 3.2; simulation.md 7.1); holds no `Rc`, `Arc` with interior mutability, pointer, function
or trait object; is not recursive; uses only the containers of 3.3. Enforcement, so none of it rests
on discipline (charter 3.7):

- `bevy_ecs` is built without its `serialize` feature, so `Entity` has no `Serialize` and a
  persisted type holding one does not compile; `serde_json` is built without `preserve_order`. The
  `deps` step (checks.md 5.2) checks resolved features.
- Registration traces each type with `serde-reflection`, whose documentation lists `flatten` and
  `tag` as unsupported; a failed trace, an incomplete format or a recursive type (found when the
  format is resolved, versions.md 3.5) refuses the type with `persist.type`.
- The encoder counts the fields each `serialize_struct` writes and fails with `persist.encode` when
  fewer than announced, which is what `skip_serializing_if` produces.
- The round-trip checks (P1 to P3) catch the rest.

**World-state types are not wire types.** The shared contract declares world state of its own
(`Controls`, `IntentTable`, `ObserverMemory`, `ObserverEvents`, `TurnState`;
shared/contract/README.md, Conventions, World state and wire forms). Its listings give their
content, with parts whose wire forms use untagged and internally tagged enums, omit `None` fields
and hold `Problem.detail` as a `serde_json` map, none of which this section allows. Each line
persists that content in its own form (the contract shares no byte formats); this line's forms are:

| Contract type | Persisted form |
|---|---|
| `ControlValue` | Externally tagged enum: `Bool(bool)`, `Number(f64)`, `Name(String)` |
| `FactValue` | Externally tagged enum: `Bool(bool)`, `Number(f64)`, `Text(String)`, `Position(Vec3)` |
| `ResolvedTarget` | Externally tagged enum: `Entity(EntityId)`, `Point(Vec3)` |
| `TurnPhase` | Externally tagged enum, without `tag` |
| An intent's failure (`IntentInstance.failure`) | `Option<StoredProblem>` with `StoredProblem { code: String, detail: PlainData }`; the message is rendered from the code's template only when the failure is projected, so no English text enters the world hash |
| `IntentInstance`, `MemoryEntry`, `PerceivedEvent`, `Reading` and every type inside the contract's world state | Every `Option` encoded, every enum externally tagged, free-form data as `PlainData` (3.4) |

Conversion to the contract's JSON shapes happens only at the interface, when an answer or a tool
shows the value (`pocket-interface`'s projection), never inside a tick. P1 covers each type of this
table.

### 3.6 Decoding is strict, and why not an existing format

The decoder accepts exactly what the encoder produces: a `bool` other than 0 or 1, invalid UTF-8, a
`char` outside the scalar values, a non-minimal ULEB128, a variant index out of range, unsorted or
repeated `PlainData` keys and bytes left over all fail with `persist.noncanonical`. So
`decode(encode(v)) == v` and `encode(decode(b)) == b` for every accepted `b`.

Rejected: bincode 1.x (close, but a `u64` per length and trailing bytes allowed by its
`deserialize`), postcard 1.x (varint integers, no skipped-field check), BCS (no floats, sorted
maps), CBOR and JSON (field names in every value; JSON stays the form agents read, replay.md 3.2).
About 500 owned lines state the format exactly and carry the checks of 3.5 and 3.6.

## 4. Snapshot layout

### 4.1 Sections

A snapshot is a header and sections, each identified by a `SectionKey` (the name `threads.md` 4.1
uses):

| Kind | Code | Name | Data |
|---|---|---|---|
| Entities | 0 | `entities` | `EntityAllocator`, then the live `EntityId`s as a `seq`, ascending |
| Resource | 1 | Registered name | The value |
| Component | 2 | Registered name | ULEB128 row count, then per row in ascending `EntityId`: the id, then the value |
| Cache | 3 | Registered name | Opaque bytes the cache writes (section 8) |

Sections are ordered by kind code, then name bytes. Names match `[A-Za-z][A-Za-z0-9_:.]{0,63}` and
are the registered stable name, never `std::any::type_name`, which changes with compiler versions
and module moves; project names cannot shadow engine names (script-host.md 7.3). A type with no
rows, or an absent resource, writes no section, so registering a type a world does not use leaves
its bytes and hash unchanged. Rows come from a query yielding `(&EntityId, &C)`, sorted by
`EntityId` (unique, so `sort_unstable`); storage order is never used for output (simulation.md 8.1).

### 4.2 Bytes

```text
snapshot   = magic format header count section* world_hash
magic      = "P3DSNAP" 0x00
format     = u32 LE, 1 (versions.md 3.4)
header     = ULEB128 length, PCE(SnapshotHeader)
count      = ULEB128
section    = kind:u8 name:PCE-str version:u32-LE fingerprint:16 bytes length:ULEB128 data
world_hash = 16 bytes, recomputed and compared on parse
```

`version` and `fingerprint` are the type's schema version and fingerprint (versions.md 3.5); a cache
has version 0 and its identity's fingerprint (versions.md 3.6). The header (tick, writes, engine
version, bundle hash) does not enter the world hash. An in-memory snapshot is read by the engine
that wrote it, whose registry keeps the format of every version it has registered; a snapshot that
leaves the process is written as a save, which carries its formats (versions.md 4, 6.1).

## 5. The world hash

### 5.1 Definition

```text
section_digest = XXH3-128(seed 0x5033_4453_4543_0001, kind:u8 || PCE(name) || version:u32-LE
                          || data)
world_hash     = XXH3-128(seed 0x5033_4457_4f52_0001, format:u32-LE || ULEB128(count) || digests)
```

`digests` is every section digest in section order. A 128-bit result is stored as its 16
little-endian bytes and shown as 32 hex digits. XXH3-128 (xxHash's 128-bit XXH3, output fixed by its
specification since xxHash 0.8.0, identical on every platform) comes from `xxhash-rust`'s
`xxh3_128_with_seed`; the seeds (`P3DSEC`, `P3DWOR` and 1) keep the two uses apart. The digests make
the hash a two-level tree: comparing section digests names the differing sections without decoding
them.

Content hashes (`ContentHash`: bundles, data files, engine sources; versions.md 3) are BLAKE3-256
through `blake3::derive_key` with a dated context string per use. They are computed when content
loads, not per tick, and they name content that travels between machines, where a cryptographic hash
keeps two different bundles from sharing a name.

### 5.2 What it covers

Every section: entities and allocator (so a fork's next `EntityId`), every resource (the clock, the
world seed, from which every random stream derives, rng.md 3), every component row and every cache,
the physics contacts and warm-start impulses included (charter 7.6). Not the header, Derived and
Ignored types, the script VM, or anything outside the world. Equal hashes mean equal persisted
state. The bundle hash is not covered, so worlds run by old and new scripts can be compared; each
section's schema version is, so a world whose component changed version differs, as it should.

Entity ids are hashed as they are. Master hashed references by an entity's place in the world's walk
because a web build created fewer hidden entities before the scene (master
`docs/design/networking.md`, Determinism); here only the simulation allocates ids (simulation.md
7.2) and the cross-target check (checks.md 7.2) holds builds to the same ids.

### 5.3 The tick hash

Replays and comparisons record per tick the `TickHash`: the world hash taken after tick `t` and
before any write of boundary `t` (simulation.md 4.2). The `EventInbox` section holds the events tick
`t` emitted (2), so the hash of tick `t` covers them and a divergence in events shows at the tick
that emitted them (simulation.md 5.2), while the world hash stays a function of the world alone:
`world_hash(fork(a)) == world_hash(a)`. The physics spike's control run shows why the hash covers
the whole state: its bodies-only hash agreed again on 278 of 293 ticks after the state diverged.

### 5.4 Cost

Each section is encoded into a reusable buffer and hashed in one call; small writes into a hasher
cost several times more. Measured 2026-10-03 on the reference laptop (AMD Ryzen 9 270), Rust 1.98.1
release, `blake3` 1.8.7, `xxhash-rust` 0.8.19, pseudo-random bytes hashed in a loop for 0.3 s, time
per call; natively, and in headless Chrome through `tools/webcheck.py` as `wasm32-unknown-unknown`
(`wasm-bindgen` 0.2.129), plain and with `+simd128` and `blake3`'s `wasm32_simd`:

| Input | BLAKE3 | BLAKE3, 8-byte updates | XXH3-128 | XXH3-128, 8-byte updates |
|---|---|---|---|---|
| 64 KiB native | 8.5 µs (7.7 GB/s) | 116 µs | 1.9 µs (35 GB/s) | 34 µs |
| 1 MiB native | 132 µs (7.9 GB/s) | 1766 µs | 31 µs (34 GB/s) | 529 µs |
| 64 KiB web | 75 µs (0.88 GB/s) | | 5.5 µs (12 GB/s) | |
| 1 MiB web | 1766 µs (0.59 GB/s) | | 115 µs (9.1 GB/s) | |
| 64 KiB web, SIMD128 | 35 µs (1.9 GB/s) | | 3.5 µs (19 GB/s) | |
| 1 MiB web, SIMD128 | 551 µs (1.9 GB/s) | | 55 µs (19 GB/s) | |

Per-tick hashes run on native and web alike (charter 4.1) and only detect accidental differences,
which a 128-bit digest makes negligible; XXH3-128 costs a quarter of BLAKE3 natively and a tenth to
a fifteenth on the web, so it is the per-tick hash, and a world's hash costs about what encoding it
does (section 13 has the slice 0 figures: the physics spike's byte-wise FNV-1a loop, not encoding,
was most of its 21 µs whole-state hash). Recording, checks and lockstep hash every tick; interactive
play without a recorder does not, as on master (`docs/design/world-model.md`, Determinism and
hashing).

## 6. Rust API

### 6.1 Types

```rust
// Copy + Eq + Ord + Hash + Debug + Serialize + Deserialize
pub struct WorldHash(pub [u8; 16]);
pub struct SectionDigest(pub [u8; 16]);
pub struct ContentHash(pub [u8; 32]);             // BLAKE3-256 (versions.md 3)
pub type TickHash = WorldHash;                    // the world hash after a tick (5.3)
#[repr(u8)] pub enum SectionKind { Entities = 0, Resource = 1, Component = 2, Cache = 3 }

// Clone + Eq + Ord + Hash + Debug + Serialize + Deserialize; ordered by kind, then name bytes
pub struct SectionKey { pub kind: SectionKind, pub name: String }

pub struct SnapshotHeader {
    pub tick: Tick,                 // SimClock's tick at the boundary
    pub writes: u32,                // writes applied at that boundary first (simulation.md 4.2)
    pub engine: EngineVersion,      // versions.md 3.1
    pub bundle: ContentHash,        // the bundle loaded at the boundary (versions.md 3.2)
}
/// Kept current by pocket-runtime, read into the header; class Ignored ("header metadata").
pub struct SnapshotContext { pub bundle: ContentHash, pub writes: u32 }   // a Resource

/// Immutable, cheap to clone, Send + Sync. Each section's bytes are an Arc<[u8]>, so the
/// publication of threads.md 4.1 can share them.
#[derive(Clone)]
pub struct Snapshot { /* header, Arc<[SectionData]>, WorldHash */ }
pub struct SectionData {
    pub key: SectionKey, pub version: u32, pub fingerprint: Fingerprint,
    pub digest: SectionDigest, pub bytes: Arc<[u8]>,
}
impl Snapshot {
    pub fn header(&self) -> &SnapshotHeader;
    pub fn world_hash(&self) -> WorldHash;
    pub fn sections(&self) -> &[SectionData];
    pub fn section(&self, key: &SectionKey) -> Option<&SectionData>;
    pub fn to_bytes(&self) -> Vec<u8>;                                   // the layout of 4.2
    /// Checks magic, format, framing, order and the trailing hash; decodes no section.
    pub fn from_bytes(bytes: &[u8]) -> Result<Snapshot, Problem>;
}
```

### 6.2 Registration

```rust
// Declared in pocket-sim (architecture.md 4.1); contracts defined here.
pub trait Persisted: Serialize + DeserializeOwned + Send + Sync + 'static {
    const NAME: &'static str;   // stable section name (4.1)
    const VERSION: u32;         // schema version from 1 (versions.md 3.5)
    /// Default: `trace_simple_type::<Self>()`; nested enums traced too (serde-reflection docs).
    fn trace(t: &mut serde_reflection::Tracer, s: &serde_reflection::Samples)
        -> serde_reflection::Result<()>;
}
pub trait PersistedCache: Send + Sync + 'static {
    const NAME: &'static str;
    fn identity() -> &'static str;                                        // versions.md 3.6
    fn encode(world: &World, out: &mut Vec<u8>) -> Result<(), Problem>;
    fn decode(bytes: &[u8]) -> Result<Box<dyn Staged>, Problem>;
    fn rebuild(world: &mut World) -> Result<(), Problem>;             // versions.md 6.3
}
/// A decoded, validated section not yet applied.
pub trait Staged: Send { fn apply(self: Box<Self>, world: &mut World); }

pub struct RegistryBuilder { /* ... */ }
impl RegistryBuilder {
    pub fn entities(&mut self) -> &mut Self;                       // EntityAllocator + live ids
    pub fn resource<R: Persisted + Resource>(&mut self) -> &mut Self;
    pub fn component<C: Persisted + Component>(&mut self) -> &mut Self;
    pub fn cache<K: PersistedCache>(&mut self) -> &mut Self;
    pub fn derived<T: 'static>(&mut self, reason: &'static str) -> &mut Self;
    pub fn ignore<T: 'static>(&mut self, reason: &'static str) -> &mut Self;
    pub fn rebuild(&mut self, name: &'static str, f: fn(&mut World)) -> &mut Self;  // after restore
    pub fn dynamic(&mut self, entry: Box<dyn DynamicEntry>) -> &mut Self;           // project types
    pub fn build(self) -> Result<Registry, Problem>;   // persist.type, persist.duplicate_name
}
```

`DynamicEntry` carries a project component's name, version, a `serde-reflection` `Format` built from
its `ComponentSchema` (script-host.md 7.3) and column encode, decode and apply functions; its bytes
are the PCE of a struct of the declared fields in declared order, each field encoded as its Rust
counterpart in script-host.md's field table. Each type is traced in its own session; within one type
serde-reflection cannot tell apart two containers with one base name or two instantiations of one
generic, and `#[serde(rename)]` resolves both. At each `snapshot` or `world_hash`, the registry
checks the component types of archetypes created since its last check, so a steady world pays
nothing; a type in no class fails with `persist.unclassified` naming it.

### 6.3 Operations

```rust
pub fn snapshot(world: &World, reg: &Registry) -> Result<Snapshot, Problem>;
pub fn world_hash(world: &World, reg: &Registry) -> Result<WorldHash, Problem>;
pub fn section_digests(world: &World, reg: &Registry)
    -> Result<Vec<(SectionKey, SectionDigest)>, Problem>;

pub struct RestoreOptions { pub verify: bool }        // default true
pub struct RestoreReport { pub tick: Tick, pub world_hash: WorldHash, pub entities: u64 }
pub fn restore(world: &mut World, snap: &Snapshot, reg: &Registry, opts: RestoreOptions)
    -> Result<RestoreReport, Problem>;

/// restore(fresh(), snapshot(world)). `fresh` builds an empty world with the schedule and the
/// non-persisted resources (spec-sim's world builder). The result can move to another thread
/// (threads.md 6).
pub fn fork(world: &World, reg: &Registry, fresh: impl FnOnce() -> World)
    -> Result<World, Problem>;

pub fn diff(a: &Snapshot, b: &Snapshot, reg: &Registry, limit: usize)
    -> Result<SnapshotDiff, Problem>;                 // replay.md 3.2
```

### 6.4 Contracts

- **Boundary.** All operations run on the game thread at a boundary (charter 5.1; simulation.md
  4.1); inside a tick they fail with `persist.not_at_boundary`. Requests from agents or the editor
  arrive as commands (threads.md 5).
- **Pure reads; same state, same bytes.** `snapshot`, `world_hash` and `section_digests` change
  nothing. Worlds with equal persisted state give identical sections and world hash, whatever their
  spawn history, storage layout or allocation order.
- **Restore is atomic for errors.** `restore` parses, checks every section's version and fingerprint
  against the registry (`persist.version` on any difference; older data goes through versions.md 6.2
  first) and decodes every section into `Staged` values before touching the world. Then it despawns
  every entity with an `EntityId`, removes every persisted resource, cache and Derived type, applies
  the sections in order, and runs the rebuild functions (`EntityIndex` first, simulation.md 7.5). On
  validated input these steps cannot fail; a failure is an engine bug and panics.
- **Restore is checked.** With `verify` (one hash pass) a world hash that differs from the
  snapshot's fails with `persist.restore_mismatch`, naming the differing sections.
- **Identity; nothing shared.** Each entity gets back its `EntityId` (`bevy_ecs::Entity` values
  differ, unobservably, simulation.md 7.1). A fork shares no `Arc` with interior mutability, cache
  or scratch buffer with its source. The bundle is a binding the runtime keeps per world: a fork
  inherits its source's at the fork and runs it in a script host of its own thread's (versions.md
  7.5; script-host.md 9).

## 7. Forks: what must match

Charter 3.3: after a fork, entity IDs, RNG state and every simulation cache MUST match. For
`b = fork(a)` at a boundary:

| What | Requirement |
|---|---|
| Persisted state | `snapshot(b)` equals `snapshot(a)` byte for byte, so the hashes are equal |
| Entity ids | Same live set and `EntityAllocator`: the next spawn gets the same id on both |
| RNG | Same world seed and clock, hence the same streams (rng.md 3) |
| Physics | Bodies, colliders, joints, contact manifolds with warm-start impulses, islands, sleep counters, broad-phase tree: the same cache bytes |
| Time model | The same resource state |
| Future | Under the same writes, the same `TickHash` at every later tick |
| Independence | Running `b` leaves `a`'s state and every later tick of `a` unchanged |

Not required to match: `bevy_ecs::Entity` values, storage layout, capacities, the script VM's heap,
render state, sessions (shared/contract/mcp.md 3) and the event history consumers keep outside the
world, and the recorder (a fork starts its own, `parent` set, replay.md 2.5). Observers' memory and
perceived events are components and do match (shared/contract/perception.md). checks.md 8.3 proves
the table; P2 and P3 prove it through bytes.

## 8. Caches: physics

The physics spike settles Rapier (`docs/spikes/physics.md`; charter 4.3, proposed in
[README.md](README.md)). Any integration MUST: keep its whole cross-tick state in one Cache section,
written by the PCE serializer from the library's deterministic `serde` output; continue after
encode, decode and restore exactly as without them (P3); find each body's entity through an
`EntityId` kept in the library's own serialized state (the reverse map is Derived); and be
rebuildable, inexactly, from the Component sections, so a save survives a library upgrade
(versions.md 6.3). simulation.md 8.5 explains why the solver's state is carried and not rebuilt: its
internal order depends on a history of insertions and removals that a rebuild in id order does not
reproduce.

For `rapier3d` 0.36.0 with `parry3d` 0.31.1, read from their sources:

- The cache value is `rapier3d::pipeline::PhysicsWorld`, which derives `serde` and skips
  `physics_pipeline`, `collision_pipeline` and `ccd_solver` as workspace
  (`src/pipeline/physics_world.rs`). Features: `serde-serialize`, `enhanced-determinism` (parry's
  maps become `IndexMap` with a fixed hasher, 3.3), `dim3`, `f32`; `parallel` and `simd8` off
  (architecture.md 4.4). A fork goes through bytes (6.3); cloning the world's parts with new
  pipelines also works (the physics spike, open choice 2).
- Rapier engineers for snapshot determinism: the broad phase serializes `prev_updated_leaves` "for
  determinism after snapshot restore" (`src/geometry/broad_phase_bvh/mod.rs`); manifolds keep
  `solver_contacts` because skipping them would "break post-snapshot determinism"
  (`src/geometry/contact_pair.rs`). Several workspaces are skipped and rebuilt on the first step
  after deserialization (`IslandManager`, `src/dynamics/island_manager/persistent.rs`,
  `bootstrapped`). Whether the rebuilt workspaces act exactly as the incremental ones is P3's
  question. For the sailing scene the physics spike answered yes: forks restored from bytes at every
  tick matched the reference on 29,910 of 29,910 ticks, natively and in Chrome, and a native
  snapshot restored in Chrome to the same bytes (`docs/spikes/physics.md`, Snapshot and fork). That
  scene had no joint, CCD, fixed or sleeping body, and Rapier also skips the pending joint state,
  `solver_clusters_prev` and `deferred_optimize_pending` (its verification), harmless only for a
  snapshot taken right after a step, as boundary snapshots are; P3's physics fixture still owes it.
- `EntityId` goes into `RigidBody::user_data` and `Collider::user_data` (`u128`), which Rapier
  serializes. Shapes serialize by value (`SharedShape` writes its typed shape, parry3d
  `src/shape/shared_shape.rs`), so an island's heightfield or a hull's triangle mesh is copied into
  every snapshot, keyframe and fork (section 13: colliders with their shapes were 9,950 of the
  physics spike's 23,764 bytes). Open choice 3 covers the case where shapes dominate fork time.
- Rapier computes some mass properties with the platform's math natively; numeric.md 5's rules keep
  that out of the cache.
- The section is opaque to `diff`; the `Transform` and velocity components written back show where
  bodies went apart.

Buoyancy, wind and hydrodynamics are systems over the bodies (charter 4.3); what they keep across
ticks (a gust phase) is a Resource or Component, and the sea surface, a function of the tick and
persisted parameters, is Derived.

## 9. Replays

Replays, recording, seeking and the session history are specified in [replay.md](replay.md), 2.

## 10. Divergence

The first diverging tick, `diff`, `lockstep` and `verify` are specified in [replay.md](replay.md), 3.

## 11. Errors

Codes travel in spec-contract's error protocol; `replay.*` are in replay.md 4, `version.*` and
`migrate.*` in versions.md 9.

| Code | When | `detail` |
|---|---|---|
| `persist.format` | Bad magic, unsupported format, unknown section kind | `{expected, found}` |
| `persist.truncated` | Bytes end inside a structure | `{offset}` |
| `persist.noncanonical` | A rejection of 3.6; sections out of order or repeated | `{offset, reason}` |
| `persist.hash_mismatch` | A parsed snapshot's trailing hash differs from its sections | `{expected, actual}` |
| `persist.unclassified` | The world holds a type in no class | `{type_name}` |
| `persist.type` | A type cannot be traced or breaks 3.5 | `{name, reason}` |
| `persist.duplicate_name` | Two types under one name | `{name}` |
| `persist.encode` | A value cannot be written (no length, skipped field, non-finite `PlainData`) | `{section, entity, reason}` |
| `persist.unknown_section` | Restore meets a section the registry lacks | `{section}` |
| `persist.version` | Restore meets another version or fingerprint | `{section, snapshot, engine}` |
| `persist.orphan` | A row names an `EntityId` not in the live set | `{section, entity}` |
| `persist.not_at_boundary` | Called while a tick runs | `{tick}` |
| `persist.restore_mismatch` | A restored world's hash differs from its snapshot's (an engine bug) | `{expected, actual, sections}` |

## 12. Checks

The charter 3.7 checks (determinism same-process, cross-process and cross-target; fork consistency;
replay with localization; reload equivalence; negative controls) are specified in checks.md 7.2 and
8, over this specification's `lockstep`, `verify`, `first_divergence` and `diff`. Persistence adds
these (checks.md 4, `test` and `projects`), over three fixtures: the headless sailing scene
(`sailboat`) under a seeded steering bot; a world that spawns and despawns 10,000 entities; a
physics world with a sleeping stack, a joint made at the boundary before the snapshot, a body under
CCD and resting contacts:

- **P1, PCE.** Property tests (`proptest`) over every persisted engine type, `PlainData`, and a test
  type covering each data-model case: `decode(encode(v)) == v`; `encode(decode(b)) == b` for
  accepted bytes; a test per rejection of 3.6; a `skip_serializing_if` field caught by the field
  count; every persisted form of the contract's world state in 3.5's table (`ControlValue`,
  `FactValue`, `ResolvedTarget`, `TurnPhase`, `StoredProblem`, `IntentInstance`, `MemoryEntry`,
  `PerceivedEvent`, `Reading`); a Component field holding NaN refused with `persist.encode` while a
  Cache section holding an infinity encodes.
- **P2, snapshot round trip.** Each fixture at boundaries 0, 1, 60 and 3,000:
  `snapshot(restore(fresh, snapshot(w)))` equals `snapshot(w)` byte for byte, and
  `Snapshot::from_bytes(s.to_bytes())` equals `s`.
- **P3, restore continuation.** Each P2 snapshot restored into a fresh world runs 600 ticks in
  lockstep with the original: equal `TickHash`es every tick. This is Rapier's round-trip verdict,
  which the physics spike gave for its sailing scene (8) and which the physics fixture still owes.
- **P4, pinned vectors.** A fixed tiny world's snapshot bytes and world hash, and XXH3-128 and
  BLAKE3 test vectors, are committed; native and web builds reproduce them exactly (as master's
  `[repro]` test pinned its math library's hash).
- **P5, golden replays.** Small committed recordings (the sailing scene, the physics world, and a
  fixture in which sections appear and disappear, replay.md 5, R2) verify in `Compare` mode; a
  golden is re-recorded only deliberately, with the reason in the commit message (master
  `docs/development.md`, Tests).
- **P6, classification and build rules.** A world with every engine type snapshots without
  `persist.unclassified`, and an unregistered test type fails with it; `bevy_ecs` has no `serialize`
  and `serde_json` no `preserve_order` (checks.md 5.2).
- **P7, performance.** Release and web builds, the sailing scene and a 10,000-entity world:
  `snapshot`, `restore`, `fork`, `world_hash`, replay bytes per tick, measured and reported against
  their reference figures (budgets.md, 5, fed by section 13); no figure is a pass condition (charter
  3.10).

## 13. Numbers

Measured in slice 0 (release builds on the reference laptop, `budgets.md` 3, on a shared machine),
or chosen here. They feed the budgets `hash.sail`, `publish.sail`, `fork.*` and `restore.sail`.

| Quantity | Value |
|---|---|
| `world_hash` of the sailing scene | Not measured with PCE and XXH3-128. The physics spike's whole-state hash (bincode and FNV-1a over 23.7 KB): 20.5 to 21.0 µs natively, 22.3 to 26.3 µs in Chrome |
| `snapshot` of the sailing scene | Not measured with PCE. The physics spike's bincode serialization: 3.8 to 4.9 µs natively, 5.4 to 7.9 µs in Chrome; 23,764 bytes at tick 400 |
| `fork` of the sailing scene | Not measured through PCE. The physics spike: serialize and restore about 20 µs natively (restore 15.9 to 23.4 µs after 20 warm-up calls; the first browser restores 200 to 540 µs), by `Clone` 5.3 to 5.7 µs; in Chrome restore 17.1 to 25.7 µs |
| `fork` of a 10,000-entity world | Not measured. The threads spike encoded and hashed a 10,000-entity world of 200,064 bytes in 29.7 µs (p50) natively |
| Slice 1, `persist_bench` (release, this laptop, medians of five runs) | A crowd of 10,000 bodies (six doubles and a name each, a tenth despawned and respawned; 765,431 bytes): `world_hash` 0.87 ms, `snapshot` 0.98 ms, `from_bytes` 0.31 ms, `restore` with `verify` 4.3 ms, `fork_into` 5.6 ms. A world of 30 bodies (2,659 bytes): `world_hash` 4.2 µs, `snapshot` 5.1 µs, `restore` with `verify` 16.4 µs, `fork_into` 20.5 µs. Encoding, not XXH3, is most of a hash (about 0.9 GB/s against 35 GB/s for XXH3 alone); restore is mostly spawning and inserting |
| The physics section's size | 18,656 bytes of Rapier state in the physics spike's bincode at tick 400 (8); not measured in PCE |
| Replay growth with section digests, without keyframes | Not measured |
| Defaults chosen here | `keyframe_every` 600 ticks (10 s at 60 Hz: a seek re-simulates at most 600 ticks, and the physics spike ran 3,000 ticks of its scene with a hash each in 145 to 148 ms natively); dense history 3,600 ticks |
| Check lengths | 600 ticks after a restore (P3, V4, V8), as `checks.md`'s `check.toml` runs; 3,000 ticks for the long snapshot (P2), the physics spike's course |

## 14. Open choices

1. **One hash function or two.** Specified: XXH3-128 per tick, BLAKE3 for content (5.1, 5.4). BLAKE3
   everywhere drops a dependency at 4 times the per-tick cost natively and 10 to 15 times on the
   web; acceptable only if the sailing scene's hash with BLAKE3 stays inside the budget `hash.sail`
   on the web (`budgets.md`). Changing it bumps the format.
2. **Fork by cloning.** architecture.md 4.1 lists a clone hook for fork. Recommended: fork through
   bytes in slice 1, one path every check exercises; a clone path (Rapier's parts cloned one by one,
   which the physics spike measured at 5.3 to 5.7 µs against about 20 µs through serde) MAY follow
   if `fork.sail` measures above its reference figure, checked by
   `snapshot(clone_fork(w)) == snapshot(w)`.
3. **Shapes by reference.** Immutable heightfields and meshes in a content-addressed table shared
   between forks by `Arc`, the cache holding their hashes. Only if the physics section's size makes
   forks exceed their reference figures (colliders with their shapes were 9,950 of the physics
   spike's 23,764 bytes); it needs a serializer around Rapier's colliders.
4. **Reusing section digests** of sections that cannot have changed: only if the per-tick hash
   measures too slow, with a full recomputation every N ticks as its check.
5. **Compression.** None in slice 1; `lz4_flex` (pure Rust, builds for `wasm32`) per keyframe and
   save if sizes matter.
6. **Section digests in replays by default.** Recommended: naming the diverging component from a
   file is worth the bytes (`replay.size.sail`, not measured in slice 0); long human matches MAY
   record `World` only.
7. **A point inside a boundary.** Settled: `(tick, writes)`, as simulation.md 2 defines it, in
   `SnapshotHeader` and in `threads.md`'s `WorldSnapshot` alike.
8. **Slice 1: the hooks as `pocket-sim` declares them.** `Persisted` (with `trace` defaulting to
   `trace_type::<Self>` over the given samples), `PersistedCache`, `Staged` and `ContentHash`
   (`derive(context, bytes)` and an incremental `hasher(context)`) are in `pocket_sim::persisted` as
   6.1 and 6.2 give them, which puts `serde-reflection` in `pocket-sim` (architecture.md 6). Crates
   below persistence declare their types' classes through the trait `RegisterPersisted`, whose
   generic methods mirror `RegistryBuilder`'s (`entities`, `resource`, `component`, `cache`,
   `derived`, `ignore`, `rebuild`) and which `RegistryBuilder` implements;
   `pocket_sim::persisted::declare` is pocket-sim's declaration (the clock, the seed, the event
   counter and inbox and `Name` persisted; the index, RNG table, outbox, tick state, tick output,
   reserved ids and poison mark Derived, rebuilt by `rebuild_derived`; the component registry
   Ignored). `dynamic` is not in the trait: project components' rows are
   `pocket_sim::registry::ProjectValues` described by the registry's `ComponentSchema`, which
   persistence reads without the script host.
9. **Slice 1: tracing needs samples.** `EntityId`'s `Deserialize` refuses 0, the value the tracer
   offers, and so do the other types whose decoding checks their values (simulation-slice1.md, 15),
   so a tracer made with `persisted::tracer_config()` (samples for structs on) takes
   `persisted::record_samples(tracer, samples)` first; a type holding an enum below its top level
   traces that enum in its `trace` (`EventInbox` traces `PlainData`, which is recursive through
   `Array` and `Object` and terminates through `Null`, its variant 0). pocket-sim's test traces
   every persisted type it declares and resolves the registry.
10. **Slice 1: `PlainData` lives in `pocket-sim`** (`pocket_sim::data`), because events carry it
    below persistence; it checks its canonical form (`validate`: finite numbers, keys strictly
    ascending by bytes) and converts to and from JSON.
11. **Slice 1: the physics cache** (8). `pocket_physics::PhysicsCache`, section `physics.rapier`,
    holds the resource `Physics` (Rapier's `PhysicsWorld`) in bincode (architecture.md 11, choice
    8). With no solver in the world it writes nothing, and an empty section decodes to no solver.
    Decoding refuses trailing bytes and a body or collider whose `user_data` names no entity
    (`persist.noncanonical`). A body's `user_data` holds its `EntityId` in the low 64 bits and an
    FNV-1a hash of its `RigidBody` and `Collider` in the high 64, so the reverse map (`BodyIndex`)
    is Derived, rebuilt by `physics.index`, and the step sees a changed component without change
    detection. `rebuild` leaves an empty solver whose bodies the next `physics.step` builds from the
    components in id order. `pocket_physics::declare` persists the components `Transform`,
    `Velocity`, `RigidBody`, `Collider`, `ExternalForce`, `Floater`, `Boat`, `Sail`, `Hull`, `Sea`
    and `Wind`, the cache, the cache's state `Physics` as Derived (choice 15), and the Derived
    `BodyIndex` and `ContactLog`. Its tests prove, for the sailing scene, a fork at every tick of
    600, restores at ticks 0, 1, 60 and 600 continuing 600 ticks in lockstep, and the storage
    shuffle, through a stand-in of this specification's operations over the same declarations
    (`pocket_physics::probe`).
12. **Slice 1: `pocket-persist` as built.** Everything of sections 3 to 7 and 11 and the checks P1
    to P4 and P6 are implemented and tested with the crate's own test game (moving bodies, spawns
    and despawns, a component that comes and goes, a resource, events, a cache that holds an
    infinity) over `pocket-sim` alone: `cargo test -p pocket-persist`, debug and release. A fork
    made at every one of 300 ticks continues with the reference's hash at every later tick
    (`tests/fork.rs`). P5 (golden replays) and P7 (budgets) wait for the sailing scene and the
    check's harness; `examples/persist_bench.rs` gives the first figures (13). `sim_registry()` is a
    `RegistryBuilder` with `pocket_sim::persisted::declare` applied.
13. **Slice 1: P1 without `proptest`.** The property tests draw their values from `pocket-sim`'s
    PCG32 (2,000 values of a type covering every case of the serde data model, NaN payloads and
    signed zeros included, and 6,000 single-bit mutations of their bytes), so no crate the
    architecture's table does not list is added for one test.
14. **Slice 1: `encode(decode(b)) == b` by re-encoding.** PCE writes a map in its iteration order
    (3.3), so a decoder alone cannot see a `BTreeMap` or `BTreeSet` whose entries arrive out of
    order: it would accept them and sort them. `pce::from_bytes_canonical` decodes and re-encodes
    and refuses bytes that differ, and every typed section (resources, components, project
    components) is decoded that way, so restore refuses with `persist.noncanonical` before it
    touches the world. `PlainData` key order is checked by the types that hold it (`EventInbox`'s
    decoding, choice 9). For `skip_serializing_if`, `serde_derive` announces the reduced field count
    and calls `SerializeStruct::skip_field`, so the encoder refuses `skip_field` (the field count
    alone, 3.5, would not see it) and also checks the count.
15. **Slice 1: classification covers resources.** `bevy_ecs` 0.19 keeps each resource as a component
    of a resource entity, so the archetype check of 6.2 sees every resource type as well as every
    component type, and an unregistered resource fails with `persist.unclassified` too. The registry
    classifies `bevy_ecs`'s `IsResource` and `DefaultQueryFilters` (present in every world) and this
    crate's `SnapshotContext` as Ignored. `bevy_ecs` names types only with its `debug` feature,
    which the workspace leaves off, so the problem names the type by its `TypeId` (and the
    registry's `classes()` lists every classified type by name). A cache's own type (the
    `PersistedCache` implementor) is classified Cache and, when it is a resource, removed by
    restore; a crate that keeps the cache's state in another resource declares that type Derived,
    `derived::<T>("the state of cache X, put back by its section")`. It must not be Ignored: restore
    removes Derived and Cache types only, so an Ignored state would survive a restore or fork from a
    snapshot without the cache section (whose `decode` is never called with empty bytes, choice 16)
    and the restore's own check would then fail with `persist.restore_mismatch` after the world had
    changed (the review of slice 1 found this; `tests/snapshot.rs` proves the Derived declaration
    with a cache whose state is a resource of its own). `pocket-physics` keeps `Physics` beside
    `PhysicsCache` (choice 11) and declares it so, `.derived::<Physics>(...)` in
    `pocket_physics::declare` (added when slice 1's wave 2 was committed: without it a registry with
    physics refused a world holding `Physics` with `persist.unclassified`).
16. **Slice 1: caches.** A cache section is written when `encode` writes at least one byte; a
    snapshot without it makes restore remove the cache's own type, and `decode` is not called with
    empty bytes. `save::rebuild_caches` runs `PersistedCache::rebuild` for the caches a load listed
    (versions.md 6.3).
17. **Slice 1: recursive formats.** `PlainData` is recursive (choice 9), which 3.5 and versions.md
    3.5 forbid, so the resolved format has a back reference: `ResolvedFormat::Back(n)` names the
    container `n` levels up the containers being expanded. Fingerprints stay free of container names
    and `EventInbox` has one; any other recursive type resolves the same way rather than failing.
18. **Slice 1: project components.** Their rows are read from `ProjectValues` through the world's
    component registry (the component's layout is checked to be `ProjectValues`), each field as its
    Rust counterpart: `f64`, `i32`, `u32`, `tick` as `Tick` (a newtype of `u64`), `bool`, `entity`
    as `Option<EntityId>` (slot value 0 is `None`), an enum as its variant index with unit variants
    named, `str`, vectors and quaternions as structs of `f64` named `x`, `y`, `z`, `w`. A slot that
    does not fit its field (2.5 in an `i32`) is `persist.encode`. Restore finds a project section by
    name in the target world's registry, so the script host registers project components before a
    restore; a world without them refuses the section with `persist.unknown_section`.
19. **Slice 1: fork and the header.** Restore sets the target's `SnapshotContext` from the
    snapshot's header, so a fork's snapshot equals its source's byte for byte, header included.
    `fork_into(world, target, reg)` forks into a world that exists already (a `Sim` owns its world
    and its systems are bound to it); `fork` is as 6.3 gives it. The entities section has version 1
    and the fingerprint of its resolved format (the allocator, then the ids). Restore despawns
    entities with an `EntityId`, removes the registered resources, the allocator, every Derived
    resource and every cache's own type, applies the sections (entities with `spawn_batch`,
    component rows with `insert_batch`), rebuilds the entity index, then runs the rebuild functions
    in registration order.
20. **Slice 1: the pinned vectors (P4)** are `pocket_persist::vectors`: XXH3-128 and BLAKE3 test
    vectors of their references, the world hash of a fixed tiny world and the BLAKE3 of its snapshot
    under a fixed header (the header names the build's target and profile, which the hash leaves
    out), and the hash after 120 ticks with a fork at tick 60 checked on the way. The native test
    writes the report and `examples/web_vectors.rs` built for `wasm32-unknown-unknown` prints the
    same bytes under Node (`node crates/pocket-persist/tests/web/run.mjs`), as `pocket-sim`'s web
    tests do.
21. **Slice 1: a bound on nesting.** A value nested more than `pce::MAX_DEPTH` (128) compound values
    deep is refused: by the encoder with `persist.encode` and by the decoder with
    `persist.noncanonical`, so neither event data a script built nor bytes from a save, a replay or
    a peer can recurse until the stack overflows, which aborts the process rather than failing (a 10
    KB section of data nested 5,000 deep did, in release). Both count alike: a sequence, tuple, map,
    struct or newtype, a `Some` and an enum variant with content each count once; `None`, a unit
    variant and scalars do not, so whatever the encoder writes the decoder reads. The JSON
    conversion of versions.md 8.1 and the field diff of replay.md 3.2 count the same way (reading on
    the decoder's counter, writing with a depth of their own), and their recursive functions keep
    one kind of value each so that frames stay small. Event data can therefore nest 62 arrays or 41
    objects inside the inbox. At that depth restore needs at most 256 KiB of stack and the JSON
    round trip and the diff at most 512 KiB in a debug build, 256 KiB in release (`tests/depth.rs`
    runs them on a 1 MiB thread). 128 follows `serde_json`'s own limit; no engine type comes near it
    (a format table entry costs three levels per level of a Rust type).
22. **Slice 1: restore checks its target first, and rows need an `EntityId`.** Before decoding,
    restore runs the class check on the target world (an unclassified type there is refused before
    the world changes, not by the verify pass after it) and the row check below, and a snapshot that
    lacks the entities section while the registry has one is `persist.noncanonical` (it parses, but
    restoring it would despawn every entity and remove the allocator). The row check
    (`Registry::check_rows`, one pass over the world's non-empty archetypes) refuses with
    `persist.encode`, naming the section, a persisted component, registered or a project component,
    on an entity without an `EntityId`: its row would be in no section and no hash, and restore,
    which despawns the entities that carry an `EntityId`, would leave it, so a fork of such a world
    would differ from its source (2: state is never skipped silently). Snapshot, the world hash and
    restore all run it; entities are spawned through `pocket_sim::entity` or a `Boundary`, which
    give them their id.
