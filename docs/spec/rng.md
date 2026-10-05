# Random numbers: PCG32 streams derived from the world seed

Status: Draft, slice 0

Charter: 3.3 (RNG state matches after a fork), 3.7 (checks), 4.2.4 (`Math.random` replaced by the
engine RNG), 7.3 (PCG32, seed derivation, streams per system or entity), 7.6 (the world hash covers
RNG state).

The simulation draws random numbers from PCG32 generators that it derives, during each tick, from
the world seed, the tick and a key naming who draws. Between ticks the only random state is the
world seed. This specification fixes the generator, the derivation, the keys, the draw operations,
what scripts get, what is hashed, and the checks. It builds on [simulation.md](simulation.md)
(ticks, boundaries, invocations) and [numeric.md](numeric.md) (the math library some draws use). The
code lives in `pocket-sim` ([architecture.md](architecture.md), 4.1).

## 1. Related specifications

| Owner | Concepts used here by name | File |
|---|---|---|
| spec-persist | the world hash, snapshot, fork, replay, the classes of world state | `persistence.md` |
| spec-script | the script host API (`ctx.rng`, `ctx.rngFor`, the `Rng` interface), the system transaction, key parts, the global freeze, the lint | `script-host.md`, `script-sandbox.md` |
| spec-contract | the error protocol, boundary writes from agents | [errors.md](../../shared/contract/errors.md), [actions.md](../../shared/contract/actions.md) |
| spec-arch | the local check command, the web build | `checks.md`, `architecture.md` |

## 2. What master and the old PocketEngine taught

- Master's game scripts drew from one stream for the whole run (`random()`, `sdk/runtime/rng.ts`).
  With one stream, a draw added anywhere moves every later draw of every system, so an edit to one
  rule changes the randomness of the whole game from that tick on, and a replay with the edited
  scripts diverges in places unrelated to the edit.
- Master had three generators: PCG32 in C++ (`engine/core/include/pocket/core/random.hpp`), sfc32
  behind the scripts' `new Rng(seed)`, and splitmix32 for scenario bots (`sdk/runtime/scenario.ts`).
  The last two kept their state inside the JavaScript VM, where saves and the state hash did not see
  it.
- Where master needed independence it keyed a stream by who draws: each scenario bot has "a
  `random()` stream of its own seeded from the run's seed and the bot's name (so a bot never
  disturbs the game's randomness and replays exactly per seed)" (`docs/design/scenarios.md`, Bots),
  and each particle emitter "owns a PCG stream seeded from its entity id"
  (`engine/renderer/include/pocket/renderer/particles.hpp`). This specification makes that the only
  way to draw.
- The old PocketEngine did not reset random seeds on reload and did not save RNG state with a save
  (PocketEngine charter, `origin/rebuild:docs/charter.md`, 8.2).
- Master's `Random::next_double` used 27 random bits; the doubles here carry 53.

## 3. Design: derived streams

A **stream** is a PCG32 generator whose starting state is a function of the world seed, the tick and
a key. During a tick the simulation keeps a table of the streams drawn from so far, so successive
draws under one key continue one sequence; the table is dropped when the tick ends (simulation.md,
4.1, invariant 3). Consequences:

1. Fork, restore and replay need no generator state, only the seed and the tick, so "RNG state ...
   MUST match" after a fork (charter 3.3) holds by construction, and the world hash covers the RNG
   by covering the seed (charter 7.6).
2. Streams are independent: a draw added to one system changes no other system's numbers and none of
   its own numbers in later ticks.
3. No generator state is stored per entity, so a crowd does not carry 16 bytes of RNG per entity in
   every snapshot and hash.
4. A script reload cannot leave a generator behind (charter 3.7, reload equivalence).
5. The price is one derivation per stream per tick, measured at 12 to 20 ns (section 11).

Deriving streams from keys instead of carrying state is established practice for reproducible
parallel and splittable randomness: the counter-based generators of Random123 (Salmon et al.,
"Parallel random numbers: as easy as 1, 2, 3", SC 2011), SplitMix's splittable streams (Steele, Lea
and Flood, OOPSLA 2014), and JAX's `jax.random.fold_in`, which derives a new key from a key and
data. The charter fixes PCG32 as the generator (7.3); the derivation supplies its seeds.

## 4. The generator: PCG32

PCG-XSH-RR with 64-bit state and 32-bit output (O'Neill 2014), exactly as `pcg32_random_r`,
`pcg32_srandom_r` and `pcg32_boundedrand_r` of the reference `pcg-c-basic`:

```rust
#[derive(Clone, Copy)]
pub struct Pcg32 { state: u64, inc: u64 } // inc is odd

const PCG_MULT: u64 = 6364136223846793005;

impl Pcg32 {
    pub fn new(initstate: u64, initseq: u64) -> Pcg32 {
        let mut r = Pcg32 { state: 0, inc: (initseq << 1) | 1 };
        r.next_u32();
        r.state = r.state.wrapping_add(initstate);
        r.next_u32();
        r
    }
    pub fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.state = old.wrapping_mul(PCG_MULT).wrapping_add(self.inc);
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        xorshifted.rotate_right((old >> 59) as u32)
    }
}
```

Reference vectors, which the implementation MUST reproduce (they match the reference `pcg32-demo`'s
output for seed 42 and sequence 54, and the spec-sim probe reproduced them on 2026-10-03, the six
32-bit values natively and in WebAssembly in Chrome, the coins and rolls natively):

- `Pcg32::new(42, 54)`, six `next_u32`:
  `0xa15c02b7 0x7b47f409 0xba1d3330 0x83d2f293 0xbfa4784b 0xcbed606e`.
- Then 65 `below(2)` (1 printed as H, 0 as T):
  `HHTTTHTHHHTHTTTHHHHHTTTHHHTHTHTHTTHTTTHHHHHHTTTTHHTTTTTHTTTTTTTHT`.
- Then 33 `below(6) + 1`: `3 4 1 1 2 2 3 2 4 3 2 4 3 3 5 2 3 1 3 1 5 1 4 1 5 6 4 6 6 2 6 3 3`.

The script-native and script-web spikes also reproduced the seed 42, sequence 54 values, natively
and in Chrome, with one PCG32 whose state they stored in the world (`docs/spikes/script-native.md`,
Determinism setup; `docs/spikes/script-web.md`, Determinism). This specification keeps their
generator and replaces the stored state with derived streams (section 3).

## 5. Derivation

### 5.1 Mixing and hashing

```rust
const GOLDEN: u64 = 0x9e37_79b9_7f4a_7c15;

/// SplitMix64's output function (Stafford's "Mix13" constants, as in Vigna's splitmix64.c).
fn mix64(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// The derived seed of a key.
fn derive(seed: u64, words: &[u64]) -> u64 {
    let mut h = mix64(seed.wrapping_add(GOLDEN));
    for &w in words {
        h = mix64(h.wrapping_add(GOLDEN) ^ w);
    }
    h
}

/// FNV-1a, 64 bits, over UTF-8 bytes.
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes { h ^= b as u64; h = h.wrapping_mul(0x0000_0100_0000_01b3); }
    h
}

/// A stream's generator.
fn stream(seed: u64, words: &[u64]) -> Pcg32 {
    let s = derive(seed, words);
    Pcg32::new(s, mix64(s ^ GOLDEN))
}
```

- `mix64` is a bijection on 64-bit words, so for a given prefix the step
  `w -> mix64((h + GOLDEN) ^ w)` is a bijection of the last word: two keys that differ only in their
  last word never share a seed. Keys that differ earlier share one only by chance, with probability
  2^-64 per pair.
- Both the initial state and the PCG sequence (`initseq`, which selects the increment) come from the
  derived seed. Two streams therefore start at unrelated points of unrelated PCG sequences; a draw
  sequence of a tick is short (typically a few numbers to a few thousand), so overlaps between
  streams are negligible.
- Test vectors (computed by the spec-sim probe in Rust and by an independent Python implementation,
  which agree): SplitMix64 seeded with 1234567 gives
  `6457827717110365317 3203168211198807973 9817491932198370423 4593380528125082431 16408922859458223821`;
  `fnv1a64` of the empty string, `a` and `foobar` is `0xcbf29ce484222325`, `0xaf63dc4c8601ec8c`,
  `0x85944171f73967e8` (the FNV reference values); `derive(0, [])` is `0xe220a8397b1dcdaf`.

### 5.2 Keys

A key is a sequence of parts. Each part is encoded as a tag word followed by its value word, so
parts of different kinds never alias (entity 17 is not the integer 17):

| Part | Tag | Value word |
|---|---|---|
| Tick | 1 | the tick number |
| System | 2 | `fnv1a64` of the system key (`script:spawner`, `physics.buoyancy`; simulation.md, 4.4) |
| Entity | 3 | the `EntityId` |
| Integer | 4 | the integer as `i64`, two's complement in a `u64`; it MUST be in ±(2^53 - 1) |
| String | 5 | `fnv1a64` of its UTF-8 bytes |

The streams a tick offers:

| Stream | Key | Use |
|---|---|---|
| System | Tick(n), System(k) | the default draws of system k in tick n, and `Math.random()` in its scripts |
| Entity | Tick(n), System(k), Entity(e) | per-entity rolls that must not change when other entities appear or vanish |
| Named | Tick(n), System(k), then any Integer, String and Entity parts | several independent purposes inside one system |
| Timeless | one or more Integer, String and Entity parts, nothing else | the same numbers whenever and by whichever system they are drawn: a level generated from its number |

Vectors for these encodings (world seed 42; first four `next_u32` of each stream; the probe's Rust
and the Python implementation agree on the encodings they share):

| Stream | Words | Derived seed | First `next_u32` |
|---|---|---|---|
| System `script:spawner` at tick 600 | `1, 600, 2, fnv1a64("script:spawner")` | `0x0a7849859791245c` | `0xdd62dfd3 0x96462238 0xd45b1aac 0xe19d0054` |
| Entity 17 of the same | the above, then `3, 17` | `0x1328745c48e6aef9` | `0xa8e403aa 0x6f5df988 0x53c07740 0x360bb58b` |
| Named `"loot", 3` of the same | the system words, then `5, fnv1a64("loot"), 4, 3` | `0x739ec45ab6687cd7` | `0xa982b9d2 0xf306d7a7 0xc212bf00 0xb4276979` |
| Timeless `"level", 7` | `5, fnv1a64("level"), 4, 7` | `0x96ce2cf498719dd7` | `0x1ab7557b 0xd595f07c 0x47498bec 0x0457fad1` |
| System `s` at tick 1, Integer -1 | `1, 1, 2, fnv1a64("s"), 4, 0xffffffffffffffff` | `0xb22f16b38242e54c` | `0xbbb70eed 0xcff5c38f 0xea49c07a 0xb3be5ab3` |

(`fnv1a64("script:spawner")` is `0x1aacefc93a5cee94`.) A timeless stream used twice in one tick
continues one sequence (5.3), and the same timeless key in another tick starts over from the same
numbers; that is its purpose.

The tags keep entity 17 apart from the integer 17 in Rust, where the two have different types. In a
script an entity is a plain number at run time, so the host could not tell the two apart: there a
number part is always an Integer part, and an Entity part is written explicitly,
`ctx.rngNamed("loot", ctx.part.entity(e))` (script-host.md 5.6), which derives the stream a Rust
system keyed by `String("loot")` and `Entity(e)` derives.

### 5.3 The per-tick table

- During a tick, asking for a stream computes its derived seed and looks it up in the tick's table
  (`BTreeMap<u64, Pcg32>` keyed by the derived seed). On a miss it creates the generator. Draws
  advance the table's entry, so two requests for the same key in one tick continue one sequence, in
  the order the requests happen, which is deterministic (simulation.md, 8).
- `sim.finish` clears the table at the end of every tick; at a boundary it is empty (simulation.md,
  4.1). It is registered as Derived (`persistence.md`, 2: rebuilt, here as the empty table, at any
  boundary), and is never serialized, hashed or iterated for a result. The RNG's persisted section
  is `WorldSeed` alone; where `persistence.md` speaks of the RNG streams' state, that section is it.
- The table supports a mark and a rollback, as the entity allocator does (simulation.md, 4.5 and
  7.2): the mark is taken when an invocation begins, the table records the previous state of each
  entry the invocation first touches, and a failed invocation's rollback restores those entries and
  removes the ones it created. The invocation's draws are then as if they never happened
  (`script-host.md`, 5.4: the host "restores the RNG streams the call drew from").
- Boundary writes cannot draw (simulation.md, 4.2): asking for a stream outside a tick fails with
  `rng.outside_tick`.

### 5.4 The world seed

```rust
/// The RNG's whole state between ticks: hashed and snapshotted with the world (spec-persist).
#[derive(Resource, Clone, Copy, Debug, Serialize, Deserialize, JsonSchema)]
pub struct WorldSeed(pub u64); // 0..=2^53 - 1
```

- The seed is set when a world is created (`SimConfig::seed`, simulation.md, 6), from the run's
  configuration; the replay header records it (spec-persist). It is limited to 0..=2^53 - 1 so it
  crosses JSON and TypeScript exactly; another value is refused with `rng.seed_invalid`.
- A **reseed** is a boundary write that replaces the seed from the next tick on, recorded in the
  replay like any write. Its use is lookahead: an agent that forks the world to try a plan against
  several random futures reseeds each fork (charter 9.2, fork-based lookahead); a fork without a
  reseed meets the same randomness as the original, which is what fork consistency requires (charter
  3.7).

## 6. Draws

A stream handle offers these operations; their results depend only on the stream's sequence. Scripts
get the same operations under names spec-script chooses.

| Operation | Definition | `next_u32` calls |
|---|---|---|
| `next_u32()` | the PCG32 output | 1 |
| `next_u64()` | `(hi << 32) \| lo`, `hi` drawn first | 2 |
| `next_f64()` | `(next_u64() >> 11) as f64 * 2^-53`: a multiple of 2^-53 in [0, 1), each equally likely | 2 |
| `below(n: u32)` | the reference `pcg32_boundedrand_r`: `threshold = n.wrapping_neg() % n`; draw `r` until `r >= threshold`; return `r % n`. `n == 0` is refused | 1 or more |
| `below_u64(n: u64)` | the same over `next_u64()` | 2 or more |
| `int(lo, hi)` | `lo + below(hi - lo + 1)`, through `below_u64` when the span `hi - lo + 1` exceeds 2^32 - 1; both ends inclusive, integers in ±(2^53 - 1), `lo <= hi` | 1 or more |
| `range(lo, hi)` | `lo + (hi - lo) * next_f64()`; finite `lo <= hi`; the result lies in [lo, hi] and equals `hi` only through rounding | 2 |
| `chance(p)` | `next_f64() < p`; `p` finite (at most 0 never, at least 1 always) | 2 |
| `pick(len)` | `below(len)` as an index; `len == 0` is refused | 1 or more |
| `shuffle(items)` | Durstenfeld's Fisher-Yates: for `i` from `len - 1` down to 1, `j = below(i + 1)`, swap `i` and `j` | `len - 1` or more |
| `fill(out)` | `out[i] = next_f64()` for `i` from 0 up | `2 * len` |
| `weighted(weights)` | weights finite and at least 0 with a positive total, summed in index order; `r = next_f64() * total`; the first index whose running sum exceeds `r`, or the last positive weight if rounding passes them all | 2 |
| `normal(mean, sd)` | Box-Muller with numeric.md's `math::ln`, `math::sqrt` and `math::cos`: `u1 = 1 - next_f64()` (in (0, 1]), `u2 = next_f64()`, `mean + sd * sqrt(-2 ln u1) * cos(2 pi u2)`; the sine half is discarded so no state is kept | 4 |

Every operation is written with exactly rounded operations and the deterministic math library
(numeric.md), so its results are the same on every target. Refusals use `rng.bound_invalid` (section
9).

## 7. Scripts

- `Math.random()` returns `next_f64()` of the invocation's system stream (charter 4.2.4).
  Spec-script's global setup installs it and freezes `Math`.
- The host offers the system stream as `ctx.rng`, the entity stream as `ctx.rngFor(entity)`, and the
  named and timeless streams of section 5.2 as `ctx.rngNamed(...parts)` and
  `ctx.rngTimeless(...parts)` (`script-host.md`, 5.1 and 5.6), with the operations of section 6 (its
  `Rng` interface: `next`, `int`, `range`, `chance`, `pick`, `shuffle`, `weighted`, `normal`,
  `fill`). Number parts are Integer parts and entity parts are made with `ctx.part.entity(e)` (5.2).
  `ctx.rngFor(e)` derives the entity stream from the id whether or not an entity holds it, and
  refuses only a value that is not a valid `EntityId` (`rng.key_invalid`). It offers no generator
  object that a script seeds and keeps: master's `new Rng(levelSeed)` becomes a timeless stream
  keyed by the level seed, whose numbers are the same whenever they are drawn.
- A stream handle is valid during the invocation that obtained it. Using one after it returns (kept
  in a closure or a module binding, which the frozen global and the lint make hard; spec-script)
  fails with `rng.stream_expired`.
- A script system's key is its key in the schedule (`script:<name>`, simulation.md, 4.4). Renaming a
  system changes its numbers, as editing its draws would; reloading it unchanged changes nothing.

## 8. Outside the simulation

Presenters (particle effects, visual jitter, audio variation), the benchmark harness, and scenario
bots that act from outside the world use their own generators and never `pocket-sim`'s streams;
nothing outside a tick can draw from them (5.3). A bot affects the world only through boundary
writes, which the replay records, so its own randomness never needs to be in the world.

## 9. Error codes

| Code | When | `detail` |
|---|---|---|
| `rng.outside_tick` | a stream is requested at a boundary | `{tick}` |
| `rng.stream_expired` | a stream handle is used after its invocation returned | `{key}` |
| `rng.bound_invalid` | `below(0)`, `pick` of nothing, `int` with `lo > hi` or ends outside ±(2^53 - 1), `range` with non-finite or reversed ends, `weighted` without a positive total | `{op, ...the arguments}` |
| `rng.key_invalid` | a key part that is not an integer in ±(2^53 - 1), a string without a lone surrogate or an entity id; `rngFor` of a value that is not a valid `EntityId` | `{part, index}` |
| `rng.seed_invalid` | a world seed outside 0..=2^53 - 1 | `{seed}` |

## 10. Checks and tests

1. **Reference vectors** (`rng.vectors`): the PCG32, SplitMix64, FNV-1a and derivation vectors of
   sections 4, 5.1 and 5.2, in the native build and in the web build in headless Chrome
   (`checks.md`, web).
2. **Empty at boundaries** (`rng.boundary`): the table is empty at every boundary (simulation.md,
   4.1, asserted by `step`).
3. **Isolation** (`rng.isolation`): a fixture project run twice, the second time with extra draws in
   one system; every other system's draws are identical at every tick.
4. **Fork and reseed** (with spec-persist's fork-consistency check): a project whose rules draw
   randomly forks, and the same writes on both sides give the same hashes; a reseeded fork diverges
   from the next tick while the original's hash chain is unchanged.
5. **Distribution** (`rng.distribution`, unit tests): `below(6)` over 6,000,000 draws per stream
   kind passes a chi-square test at the 0.001 level; `next_f64` over 1,000,000 draws has a mean
   within 0.002 of 0.5.
6. **Derivation quality** (once in slice 1 and again whenever the derivation changes): the first
   `next_u32` of the entity streams of ids 1 to 2^32 at one tick, concatenated, and the first
   `next_u32` of one system stream over 2^32 consecutive ticks, each 2^34 bytes fed to PractRand
   (`RNG_test stdin32`), with no failure. The ids and ticks are only key values; no world is needed.
   PCG32's own sequence quality is published (O'Neill 2014 reports it passing TestU01's BigCrush);
   what this test adds is the independence of derived streams, which is this design's own.

## 11. Performance

| Quantity | Value | How |
|---|---|---|
| Deriving a stream over four key words, seeding PCG32 and drawing one `next_u64` | 11.8 to 19.9 ns | spec-sim probe, release, x86_64, three runs |
| One `next_u32` | 1.03 to 1.37 ns | spec-sim probe, three runs |
| Entity streams for 10,000 entities in one tick | not measured in slice 0 | slice 1 bench `rng.entity_streams` |
| A draw from a script, including the host call | not measured in slice 0: the script-native spike drew from one stored PCG32 (`randomFill` in bulk, `Math.random` per call) and did not time a draw; its host calls into Rust `Math` cost 93 to 214 ns | slice 1 bench of the script host |

The probe ran on the reference laptop under the conditions of simulation.md, 13.

## 12. Open choices

1. **Derived streams or stored generator state.** Recommendation: derived, for the five reasons of
   section 3. Stored per-system and per-entity generators (master's C++ `Random`, held in the world)
   also satisfy fork consistency, but every stream becomes hashed and snapshotted state, and a draw
   added to a system shifts that system's numbers for the rest of the run.
2. **The string hash.** Recommendation: FNV-1a 64. Keys are not adversarial and a collision has
   probability 2^-64 per pair; SipHash or xxHash3 would add a dependency or a longer specification
   for no gain here.
3. **Bounded integers.** Recommendation: the reference threshold method, which the published vectors
   cover. Lemire's multiply-shift method ("Fast Random Integer Generation in an Interval", 2019)
   avoids most divisions, which this workload does not need.
4. **Boundary writes and randomness.** Recommendation: writes never draw (simulation.md, 14, choice
   3).
5. **Seed width.** Recommendation: 53 bits, for exact transport through JSON and TypeScript. A
   string form for full 64-bit seeds is not needed: 2^53 seeds are more than any benchmark draws.
6. **Slice 1: the table's shape.** The per-tick table maps derived seeds to generators in an
   open-addressing hash table with linear probing, cleared in constant time by a generation stamp,
   instead of the `BTreeMap` of 5.3: it offers lookups only, never iteration, so no order reaches a
   result (simulation.md 8.4), and the derived seeds, being SplitMix outputs, index it by their low
   bits. 10,000 entity streams drawn once each in a tick took 2.1 ms with the `BTreeMap` and 0.54 ms
   with the table (release, simulation.md 13). Keys are derived word by word without a buffer.
7. **Slice 1: the Rust API.** `RngTable::system`, `entity`, `named` and `timeless` return the tick's
   generator for that key, and the operations of 6 are methods of `Pcg32` (`Stream<'a>` is
   `&'a mut Pcg32`). A handle a script keeps across host calls is a `StreamKey` (the derived seed,
   from `RngTable::key`) redeemed with `RngTable::stream`; whether it has expired is the script
   host's to track, with `rng::stream_expired` for the refusal. `RngTable::mark`, `commit` and
   `rollback` are the invocation marks of 5.3, kept by `sim::begin_invocation` and its relatives.
8. **Slice 1: refusals beyond 9's list.** `chance` with a non-finite `p` and `weighted` with an
   empty list are `rng.bound_invalid`; `pick` and `shuffle` of more than 2^32 - 1 items draw through
   `below_u64`.
9. **Slice 1: not run.** The derivation-quality test (10, item 6) needs PractRand and 2^34 bytes per
   stream kind; it is not part of this slice's tests. The fork part of item 4 needs
   `pocket-persist`; pocket-sim's tests run two worlds from the same seed and writes instead, and
   show a reseed changing every stream from the next tick on.

## 13. References

- Charter: `docs/charter.md` 3.3, 3.7, 4.2.4, 7.3, 7.6.
- M. E. O'Neill, "PCG: A Family of Simple Fast Space-Efficient Statistically Good Algorithms for
  Random Number Generation" (2014), and the reference `pcg-c-basic` (`pcg32_random_r`,
  `pcg32_srandom_r`, `pcg32_boundedrand_r`, `pcg32-demo`).
- G. L. Steele, D. Lea, C. H. Flood, "Fast Splittable Pseudorandom Number Generators" (OOPSLA 2014);
  S. Vigna, `splitmix64.c`.
- J. K. Salmon, M. A. Moraes, R. O. Dror, D. E. Shaw, "Parallel random numbers: as easy as 1, 2, 3"
  (SC 2011).
- JAX, `jax.random.fold_in`.
- G. Fowler, L. C. Noll, K.-P. Vo, the FNV hash (FNV-1a).
- D. Lemire, "Fast Random Integer Generation in an Interval" (ACM TOMACS, 2019).
- Master: `docs/design/scenarios.md` (Bots), `sdk/runtime/rng.ts`, `sdk/runtime/scenario.ts`,
  `engine/core/include/pocket/core/random.hpp`,
  `engine/renderer/include/pocket/renderer/particles.hpp`.
