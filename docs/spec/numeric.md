# Numeric rules and the deterministic math library

Status: Draft, slice 0

Charter: 3.3 (the same hash on every target), 4.2.4 (trigonometry and similar functions from an
engine library), 4.2.7 (numbers are doubles; conversion and range checks into integer components),
7.2 (numeric rules).

This specification fixes which simulation state may be floating point and at what precision, the
floating-point environment the simulation assumes, which operations simulation code may use, the
deterministic math library (module `math` of `pocket-sim`, [architecture.md](architecture.md), 4.1)
with its functions and measured accuracy, how NaN, infinities and signed zeros are treated, and how
values enter integer fields. It is the numeric half of [simulation.md](simulation.md); random
numbers are in [rng.md](rng.md).

## 1. Related specifications

| Owner | Concepts used here by name | File |
|---|---|---|
| spec-script | numbers at the boundary (`script-host.md`, 6), the replaced `Math` functions (`script-sandbox.md`, 2.2), the `**` lowering (`script-host.md`, 8.2), the NaN canonicalization patch (`script-sandbox.md`, 4.2, P6) | `script-host.md`, `script-sandbox.md` |
| spec-persist | canonical encoding of floats, the world hash | `persistence.md` |
| spec-contract | the error protocol; actions whose parameters arrive as JSON numbers | [errors.md](../../shared/contract/errors.md), [actions.md](../../shared/contract/actions.md) |
| spec-arch | C compiler flags (no contraction), crate layout, the clippy step, the web check | `architecture.md`, `checks.md` |

## 2. What goes wrong without these rules

- **The platform's functions differ in the last bit.** Master found that "the platform's `sin`,
  `cos`, `atan2`, `exp`, `pow` differ in the last bit between macOS's libm and the musl in a web
  build, and between JavaScriptCore (native scripts), V8 and other engines", and built
  `pocket::repro` to compute them from `+ - * /` and `sqrt` alone (`docs/design/networking.md`,
  Determinism, "Reproducible math"). The Amoris charter counts `Math.sin` results that differ
  by platform among the old engine's failures (8.2). Measured here (section 6.4): Rust's own
  `f64::sin` and 22 other standard functions (25 of 26 sweeps) give different bits natively on
  Windows and in WebAssembly in Chrome over the same inputs; only `sqrt` agrees. The script-web
  spike found the same independently for QuickJS-ng's `Math` and Rust's `f64`
  (`docs/spikes/script-web.md`, The numeric probe: `atan2` differed for 821 of 4,096 inputs, `powi`
  for 1,644 by up to 5 ulps), because on each platform both call that platform's C library. Rust's
  documentation calls the precision of these functions non-deterministic: it "varies by platform,
  Rust version, and can even differ within the same execution".
- **`min` and `max` are not deterministic either.** Measured: `f64::max(+0.0, -0.0)` and
  `f64::min(+0.0, -0.0)` return `-0.0` in a debug build and `+0.0` in a release build of the same
  program on the same machine (section 6.4). Rust documents it: for +0.0 and -0.0 "either input may
  be returned non-deterministically". A signed zero that reaches the world hash splits debug and
  release runs.
- **Fused multiply-add.** Master's arm64 build fused `a * b + c` where WebAssembly rounds twice,
  until every C compile got `-ffp-contract=off` (`docs/design/networking.md`, Determinism). Rust
  never contracts on its own; only an explicit `mul_add` fuses, which section 5 forbids. C code
  (QuickJS-ng) is compiled without contraction ([architecture.md](architecture.md), 7.3).
- **NaN bits are not portable.** The sign and payload of a NaN produced by arithmetic are not
  specified by Rust's floating-point semantics (RFC 3514) or by WebAssembly, and x86's default NaN
  has its sign bit set where ARM's does not. A NaN that reaches the world hash can therefore differ
  between two targets that computed "the same" NaN, and so can any code that reads a NaN's bits
  within one call: `to_bits`, `total_cmp` (which orders NaNs by their sign), `is_sign_negative` and
  `copysign` in Rust, and a script that stores a NaN into a typed array and reads its bytes
  (QuickJS-ng keeps the payload on 64-bit targets and NaN-boxes, canonicalizing it, on 32-bit ones;
  script-sandbox.md 4.2, P6).
- **Someone else's control register.** Libraries and drivers have changed the floating-point mode of
  the thread that called them (Direct3D 9 lowered the x87 precision of the calling thread unless
  created with `D3DCREATE_FPU_PRESERVE`); flush-to-zero set on a thread changes every subnormal
  result. The game thread calls no driver (charter 5.1), and section 4 checks the register.
- **Scripts.** ECMAScript leaves `Math.sin` and its kind "implementation-approximated", and
  QuickJS-ng computes them with the C library it is linked with, which is not the same library
  natively and on the web (`script-sandbox.md`, 2.2).

## 3. Number types in simulation state

### 3.1 Reals are f64

Every floating-point value in persisted simulation state (components and resources,
`persistence.md`, 2) is an `f64`. Reasons:

1. Scripts compute in doubles (charter 4.2.7). With `f64` fields a value a script writes reads back
   bit for bit; with `f32` fields, `0.1` written reads back as `0.10000000149011612`, and
   `x === 0.1` after a write is false, a trap for every agent that writes game rules.
2. Range and resolution: an `f64` resolves 9.1e-13 m at 4 km from the origin, where an `f32`
   resolves 0.49 mm. Simulated time and wave phases (`k x - omega t`) grow with the run; in `f64`
   they keep their precision for any run length a game reaches.
3. One type means no conversion rules inside the simulation, except at the one place a library
   forces another (below).

`f32` appears only outside persisted state: rendering, GPU buffers and the presenters' derived data
(camera-relative positions computed on the presenter side from the snapshot, never written back).
The row `f32` in `script-host.md`'s table of field types (6) is then unused: no persisted field is
`f32`.

The physics solver is `rapier3d` 0.36.0, which computes in `f32`: the configuration the physics
spike proved deterministic natively and in the browser (`docs/spikes/physics.md`, Verdict);
`rapier3d-f64` was not measured. An `f32` solver resolves the showcase's sailing area (a few
kilometres; 0.49 mm at 4 km). The solver's state is a Cache (`persistence.md`, 8), and the
conversion between the solver and the `f64` components happens only in the physics sync and
write-back systems, by `as f32` (IEEE round to nearest, ties to even) going in and `as f64` (exact)
coming out, so the round trip is deterministic and visible in one place. The engine's force systems
(wind, buoyancy, sail, hull) compute in `f64` with `math` and hand the solver `f32` forces at the
same place; the physics spike computed them in `f32` with `libm`'s `f32` functions, which the
cross-target check covers the same way.

### 3.2 Integers

Persisted numeric field types and the range each admits:

| Field type | Rust | Admitted values | Script sees |
|---|---|---|---|
| `f64` | `f64` | finite doubles (section 7) | number |
| `i32` | `i32` | -2^31 to 2^31 - 1 | number |
| `u32` | `u32` | 0 to 2^32 - 1 | number |
| `tick` | `Tick` | 0 to 2^53 - 1 | number |
| `entity` | `Option<EntityId>` | 1 to 2^53 - 1, or none | number or `null` (0 in a column) |

Wider integers (`i64`, `u64`) are not exposed to scripts, since doubles cannot hold them exactly
(`script-host.md`, 6); engine-internal state that needs them keeps them out of script-visible
components.

What must be an integer: identities (`EntityId`), counts and indices, quantities that must add
exactly (money, inventory, score), discrete states, and durations that gate gameplay, counted in
ticks (simulation.md, 3.1).

### 3.3 Units

Simulation state uses SI units and radians: metres, kilograms, seconds, newtons, metres per second,
radians and radians per second. What units and angle conventions the agent-facing interface presents
(degrees for a heading, knots for a speed) is spec-contract's; the conversion happens at that
interface, never inside a tick. The axis and heading conventions of the physics and the sea belong
to their own specifications.

## 4. The floating-point environment

- Every target computes IEEE 754 binary64 with round to nearest, ties to even, gradual underflow
  (subnormals) and no traps: `x86_64` (SSE2), `aarch64`, and `wasm32`. A 32-bit x86 target without
  SSE2 (x87 arithmetic, extended precision) is unsupported.
- Rust's `+ - * /`, `%`, `sqrt`, the conversions and the comparisons are IEEE operations, the same
  in debug and release (measured, section 6.4); Rust does not reorder or contract floating-point
  expressions.
- The game thread's control register is checked at the start of every tick in every build (one
  register read): on `x86_64`, MXCSR with its six sticky exception flags masked out MUST equal
  `0x1F80` (all exceptions masked, round to nearest, flush-to-zero and denormals-are-zero off), read
  with `stmxcsr` through inline assembly (the `_mm_getcsr` intrinsic is deprecated); on `aarch64`,
  FPCR's rounding mode (bits 22 to 23) and flush-to-zero (bit 24) MUST be 0. WebAssembly has no such
  register. A mismatch is the internal error `number.float_env_changed` with the register's value,
  and the world is poisoned (simulation.md, 4.6).

## 5. Operations in simulation code

Allowed, because IEEE 754 or Rust defines their results exactly:

- `+`, `-`, `*`, `/`, `%` (fmod is exact; LLVM lowers Rust's `%` on floats to a call of the
  platform's `fmod`, which `math.golden` sweeps, section 10), unary `-`, `sqrt`, `abs`, `signum`,
  `floor`, `ceil`, `trunc`, `round` (half away from zero), `round_ties_even`, `fract`, `rem_euclid`,
  `div_euclid`, `recip`, `to_degrees`, `to_radians`, `midpoint`, `clamp` (defined by comparisons),
  comparisons, `from_bits`, integer-float conversions (exact, or correctly rounded beyond 2^53), and
  float formatting and parsing in Rust's standard library (correctly rounded).
- `to_bits`, `total_cmp`, `is_sign_negative`, `is_sign_positive` and `copysign` only on values known
  not to be NaN, or on the result of `math::canonical(x)`, which maps every NaN to
  `0x7FF8000000000000` (`f32`: `0x7FC00000`) and returns any other value unchanged (section 2: a
  NaN's sign and payload differ between targets).
- The functions of `pocket_sim::math` (section 6) and the `libm` crate it wraps.

Disallowed in the tick-code crates, by clippy's `disallowed-methods` (simulation.md, 10; every path
below resolves in clippy 1.98.1, checked with a scratch crate on 2026-10-03 for `f64::sin`,
`f64::powi`, `f64::max` and `f64::mul_add`, and of the same form for the rest):

```toml
# f64 and f32 alike; the f32 entries repeat these paths with f32.
{ path = "f64::sin", reason = "use pocket_sim::math::sin (numeric.md, 6)" },
# likewise: f64::cos, tan, sin_cos, asin, acos, atan, atan2, sinh, cosh, tanh, asinh, acosh,
# atanh, exp, exp2, exp_m1, ln, log, log2, log10, ln_1p, powf, powi, cbrt, hypot
{ path = "f64::min", reason = "signed zeros differ between builds; use pocket_sim::math::min (numeric.md, 5)" },
{ path = "f64::max", reason = "signed zeros differ between builds; use pocket_sim::math::max (numeric.md, 5)" },
{ path = "f64::mul_add", reason = "one rounding model: no fused multiply-add in tick code (numeric.md, 5)" },
```

- The transcendental functions of `std` have unspecified precision (2) and are replaced by `math`.
- `powi` is replaced by `math::powi`, whose multiplication order is fixed (6.2); `std`'s is an LLVM
  intrinsic whose order is not.
- `min` and `max` are replaced by `math::min` and `math::max`, defined by comparisons (6.2).
- `mul_add` is correctly rounded and so deterministic in itself, but a fused expression in a Rust
  system and its unfused twin in a script give different bits; one rounding model keeps engine and
  script arithmetic interchangeable. (The `libm` crate uses fused operations inside a few functions,
  such as `cbrt`; those are inside one implementation shared by every caller, and the golden sweep
  pins them.)
- No SIMD intrinsics, no `target-feature` that changes floating-point code generation, and no
  relaxed SIMD on the web ([architecture.md](architecture.md), 7.3).
- `as` from a float to an integer saturates silently (NaN becomes 0); the tick-code crates deny
  clippy's `cast_possible_truncation`, `cast_sign_loss` and `cast_precision_loss`, so conversions go
  through section 8's functions.

Third-party code inside a tick routes its transcendental functions through `libm` only in part.
Rapier and parry with `enhanced-determinism` force `simba`'s `libm_force` and `glam`'s `libm`
([architecture.md](architecture.md), 6), but the physics spike's verification found that once `std`
is linked (`serde-serialize` turns it on) the inherent `f32` methods of glamx 0.3.1, parry3d 0.31.1
and rapier3d 0.36.0 call the platform's C library natively: `acos` and `cos` in glamx's `eigen3.rs`
(the principal inertia of every convex hull, trimesh or compound collider given a density, and per
tick from `ccd/sweeps.rs`), `log2` in parry's `bvh_optimize.rs` (every step), `asin` and `sin_cos`
in Rapier's revolute joint and joint constraint helper, and the CCD interval code. Measured: 550 of
20,000 random convex hulls got different mass properties natively and in Chrome, and a world of 36
dynamic hulls with a density diverged from tick 0 (the verification of `docs/spikes/physics.md`).
Until those calls are patched to `libm` (a patched glamx and parry under `[patch.crates-io]`, as the
QuickJS-ng patches are carried; the patch list includes parry's `log2` in
`src/partitioning/bvh/bvh_optimize.rs` lines 26 and 27, below), slice 1 holds two rules:

- Every dynamic convex hull, trimesh or compound collider gets explicit mass properties (density 0
  and the body's `additional_mass_properties`), as the spike's hull did; balls, cuboids and
  capsules, whose mass properties are closed formulas, keep their density (the spike's crates did).
  A hull, trimesh or compound with a density is refused at load with `sim.collider_density`.
- Joints and continuous collision detection stay out of games until the cross-target check passes
  with a fixture that uses each.

parry's BVH optimization runs on every step and takes `log2` of the ceilinged leaf counts
(`target_root_node_count.log2()` and `target_subtree_leaf_count.log2()`), then rounds the result
into a subtree count, through the platform's library natively. The physics spike's one boat and six
crates matched natively and in Chrome, but agreement for other collider counts is luck until the
patch lands. Until then a unit test evaluates that expression, exactly as parry 0.31.1 writes it
(with `num_optimized_leaves = (num_leaves * 5).div_ceil(100)`), with `f32::log2` and with
`libm::log2f` for every `num_leaves` from 1 to 1,000,000 and asserts equal rounded counts, and the
`bodies` workload (200 bodies, budgets.md 4) is among the `[web] workload` projects of the
cross-target check.

The native-against-web hash comparison (`checks.md`, 7.2) is the proof that covers code the lint
cannot see. Its physics fixture holds a dynamic hull with a density, a joint and a CCD body, which
the rules above keep out of games and which call the platform's math natively, so until the glamx
and parry patch lands the fixture is a negative control: its `check.toml` says
`[expect] fail = "web.cross_target_diverged"` (checks.md 8.6). When the control starts reporting
`check.control_passed`, the patch is proved, and one commit turns the fixture into a positive
`[web] workload` and lifts both rules.

## 6. The deterministic math library

### 6.1 Implementation

`pocket_sim::math` is a set of Rust functions over `f64`. The transcendental functions wrap the
`libm` crate, pinned exactly (`libm = "=0.2.16"`), a Rust port of musl's libm (with newer algorithms
from ARM's optimized-routines and CORE-MATH in places); the rest are defined in section 6.2.

Why `libm`, measured on 2026-10-03 with the spec-sim probe (section 6.4):

- **The same bits everywhere.** Over 100,000 inputs for each of 26 sweeps, `libm` gave identical
  results natively in a debug build, natively in a release build, and in WebAssembly in headless
  Chrome.
- **Accuracy.** Within 0.5 to 1.83 units in the last place (ulp) of the correctly rounded value over
  the sampled ranges (6.2), comparable to the platform's own library.
- **It is already in the stack.** Rust's standard library computes these functions with a copy of it
  on `wasm32-unknown-unknown` (measured: `std`'s results in Chrome equal `libm` 0.2.16's for 24 of
  26 sweeps, all but `hypot` and `atanh`), and Rapier's `enhanced-determinism` routes its math
  through it.
- **Built from exactly rounded operations.** Its code uses IEEE arithmetic, `sqrt`, rounding to
  integers, scaling by powers of two, bit manipulation and, in a few functions, correctly rounded
  fused multiply-add, each of which IEEE 754 defines to the bit; its `arch` feature selects hardware
  instructions only for such operations. This is the property the charter asks for when it says
  "built from `+ - * /` and `sqrt`" (4.2.4, 7.2); open choice 2 proposes the charter's wording
  follow it.

The alternative, an `f64` port of master's `repro` (`sdk/runtime/repro.ts`), was measured on the
same inputs: it is as deterministic, but less accurate (`atan2` 4.1 ulp, `asin` 4.5, `pow` 54),
wrong for large arguments (`sin` of values near 10^9 is wrong in every digit, because its argument
reduction splits pi/2 into a 33-bit head exact only for fewer than 2^20 quarter turns), and slower
(`sin` 22.7 ns against 14.6 in the first timing run). The script-native spike used that port in Rust
(its `math.rs`, worst `pow` error 16 ulp against the MSVC library) and showed the shape this
specification keeps: one Rust library behind `Math.*`, the same bits natively and in
`wasm32-wasip1`; slice 1 puts `libm` behind that binding instead of the port.

Updating `libm` (or changing any function here) changes simulation results. It is an engine version
change (`versions.md`, 3.1): the golden sweep (section 10) is updated in the same commit, with the
reason.

### 6.2 Functions and contracts

Every function takes and returns `f64` unless noted. Special values (NaN, infinities, signed zeros,
domain edges) follow C99 Annex F as musl implements it. "Max error" is the largest error in ulp,
against values computed with 200-bit precision by mpmath, over 3,000 random inputs in the sampled
range (6.4); it describes accuracy, not the contract, which is "these bits, on every target".

| Function | Sampled range | Max error (ulp) |
|---|---|---|
| `sin(x)`, `cos(x)` | [-2 pi, 2 pi] | 0.68, 0.65 |
| `sin(x)` | [-1e5, 1e5]; [-1e9, 1e9] | 0.70; 0.66 |
| `sin_cos(x) -> (f64, f64)` | as `sin` and `cos` | as `sin` and `cos` |
| `tan(x)` | [-1.5, 1.5] | 0.67 |
| `asin(x)`, `acos(x)` (NaN outside [-1, 1]) | [-1, 1] | 0.71, 0.72 |
| `atan(x)` | [-1e3, 1e3] | 0.60 |
| `atan2(y, x)` | [-100, 100] squared | 1.14 |
| `sinh`, `cosh`, `tanh` | [-10, 10] | 1.37, 1.13, 1.83 |
| `asinh`; `acosh`; `atanh` | [-1e3, 1e3]; [1, 1e3]; [-0.999, 0.999] | 0.94; 0.70; 1.37 |
| `exp(x)`; `exp2(x)` (overflow to infinity, underflow to 0) | [-50, 50] | 0.88; 0.50 |
| `expm1(x)` | [-5, 5] | 0.72 |
| `ln(x)`, `log2(x)`, `log10(x)` (NaN below 0, -infinity at 0) | [2^-34, 2^34) | 0.69, 0.54, 0.56 |
| `ln_1p(x)` | [-0.9, 10] | 0.70 |
| `pow(x, y)` | x in [2^-7, 2^7), y in [-10, 10] | 0.81 |
| `cbrt(x)` | [-1e6, 1e6] | 0.50 |
| `hypot(x, y)` | [-1e3, 1e3] squared | 0.96 |
| `sqrt(x)` (IEEE, the hardware instruction) | [2^-20, 2^20) | 0.50 |

Defined here, with their exact operation sequence:

```rust
pub fn min(a: f64, b: f64) -> f64 { if b < a { b } else { a } }   // ties and NaN: returns a
pub fn max(a: f64, b: f64) -> f64 { if b > a { b } else { a } }   // ties and NaN: returns a
pub fn clamp(x: f64, lo: f64, hi: f64) -> f64 { if x < lo { lo } else if x > hi { hi } else { x } } // lo <= hi
pub fn lerp(a: f64, b: f64, t: f64) -> f64 { a + (b - a) * t }
/// x^n by squaring: r = 1, b = x, e = |n|; while e > 0 { if e odd { r *= b }; b *= b; e >>= 1 };
/// n < 0 returns 1 / r. Exact whenever every intermediate product is representable.
pub fn powi(x: f64, n: i32) -> f64;
/// The angle a - TAU * floor((a + PI) / TAU), in [-PI, PI) up to rounding.
pub fn wrap_angle(a: f64) -> f64;
/// Every NaN to 0x7FF8_0000_0000_0000; any other value unchanged (section 5). canonical_f32 likewise.
pub fn canonical(x: f64) -> f64 { if x.is_nan() { f64::from_bits(0x7FF8_0000_0000_0000) } else { x } }
```

Constants are the nearest doubles: `PI`, `TAU`, `FRAC_PI_2`, `E`, `LN_2`, `LN_10`, `SQRT_2` (Rust's
`std::f64::consts` values). Floating-point identities do not hold exactly: `sin(PI)` is about
1.22e-16, not 0, because `PI` is not pi. Game code compares with tolerances, or counts in integers.

### 6.3 Scripts get the same functions

- The script host binds these Rust functions as `Math.sin`, `Math.cos`, `Math.tan`, `Math.asin`,
  `Math.acos`, `Math.atan`, `Math.atan2`, `Math.sinh`, `Math.cosh`, `Math.tanh`, `Math.asinh`,
  `Math.acosh`, `Math.atanh`, `Math.exp`, `Math.expm1`, `Math.log`, `Math.log1p`, `Math.log2`,
  `Math.log10`, `Math.pow`, `Math.cbrt` and `Math.hypot`, replacing QuickJS-ng's own
  (`script-sandbox.md`, 2.2). Code written from the existing TypeScript corpus (`Math.sin(angle)`)
  therefore runs deterministically without edits, and a script and an engine system computing
  `sin(x)` get the same bits.
- `Math.hypot` takes any number of arguments: the binding computes `sqrt` of the sum of squares in
  argument order for three or more, and `math::hypot` for two; with no argument it is 0, and any
  infinite argument gives infinity, as ECMAScript requires.
- `Math.pow` and `**` (which the transpiler lowers to `Math.pow`, `script-host.md`, 8.2) use
  `math::pow`, except that ECMAScript's own special cases come first where they differ from C's
  (`Math.pow(1, NaN)` and `Math.pow(-1, Infinity)` are NaN in ECMAScript and 1 in C).
- The other `Math` functions stay QuickJS-ng's: `abs`, `ceil`, `floor`, `round`, `trunc`, `sign`,
  `sqrt`, `fround`, `min`, `max`, `imul`, `clz32` and the rest of `script-host.md`'s kept list.
  ECMAScript specifies their results exactly (`Math.sqrt` included), and QuickJS-ng computes them in
  its own code. Note that `Math.round` rounds half up (`Math.round(-2.5)` is -2) where Rust's
  `round` rounds half away from zero (-3); both are deterministic, but a rule ported between Rust
  and TypeScript must name the rounding it means.
- Numbers pass between scripts and the world as doubles on both sides; nothing is converted on the
  way except into integer fields (section 8).
- QuickJS-ng's number formatting and parsing (`String(x)`, `parseFloat`, `JSON`) is its own code,
  not the C library's `printf` and `strtod`, so it is the same natively and on the web
  (`script-host.md`, 10); the cross-target check confirms it.

### 6.4 Measurements

The spec-sim probe (2026-10-03, scratch code kept outside the repository, which slice 1's
`math.golden` and `math.accuracy` checks replace): a Rust crate that generates inputs with PCG32
(integer bit construction and `+ - *` only), evaluates each function through `libm`, through `std`,
and through an `f64` port of master's `repro.ts`, and folds the result bits into a 64-bit hash per
sweep (100,000 inputs each). Built with Rust 1.98.1 for `x86_64-pc-windows-msvc` (debug and release)
and for `wasm32-unknown-unknown` (release, loaded by a page in headless Chrome through
`tools/webcheck.py`). Accuracy: 3,000 inputs per function compared with mpmath 1.3.0 at 200 bits.
Timings: release, 1,000,000 calls per function, on the reference laptop while other agents' builds
shared it, so the three runs spread widely.

| Result | Value |
|---|---|
| `libm` sweeps equal across native debug, native release and Chrome | 26 of 26 |
| `std` sweeps in Chrome equal to `libm`'s | 24 of 26 (not `hypot`, `atanh`) |
| `std` sweeps equal between native release and Chrome | 1 of 26 (`sqrt`) |
| `repro` port sweeps equal across the three | 15 of the 15 it provides |
| Native debug against native release, all three implementations | equal, except `std` `min`/`max` on signed zeros |
| `f64::max(+0, -0)`, `f64::min(+0, -0)`: native debug; native release; Chrome | -0, -0; +0, +0; +0, +0 |
| Time per call, `libm`, ns (the loop and dispatch cost about 3 to 4 ns of each, the `sqrt` row) | `sin` 14.0 to 17.4, `cos` 15.3 to 19.2, `atan2` 21.2 to 27.9, `exp` 6.7 to 13.0, `ln` 5.6 to 9.7, `pow` 38.5 to 63.3, `sqrt` 3.1 to 4.0 |
| Time per call, `std` (Windows UCRT), for comparison | `sin` 11.8 to 14.2, `pow` 11.5 to 23.1 |

In the same runs `libm`'s `pow` took 2.7 to 3.3 times the UCRT's time and its `sin` 13 to 23% more.
A script's call into `math` costs a host call on top: the script-native spike measured an engine
`Math.sin` from a script at 120 to 140 ns a call and `Math.atan2` at 159 to 214 ns (its verification
93 to 131 and 146 to 171 ns), against 740 to 1,100 ns for the same algorithm interpreted in
TypeScript (`docs/spikes/script-native.md`, Math per call); that spike bound its own Rust port of
`repro`, not `libm`, so slice 1's bench measures the binding of this library. The library's cost in
the web build was not measured in slice 0; the web check's `perf` step measures it.

## 7. Non-finite values and signed zero

- **NaN and infinities never enter persisted state.** Every write of a float into a persisted field
  checks `is_finite`: a script's write is refused by the host (`script.write_not_finite`,
  `script-host.md`, 6), a boundary write is refused by its validation (`number.not_finite`), and an
  engine system checks what it writes. The physics write-back checks each body; when the solver
  produced a non-finite position or velocity, it reports `number.not_finite` with the entity, leaves
  the body's components as they were, and resets that body in the solver to them with zero velocity,
  which is deterministic. The canonical encoding (`persistence.md`, 3.1) writes every float bit for
  bit, non-finite values included; its typed encoders of Component and Resource sections and
  `PlainData` refuse non-finite values as a last line of defence (`persist.encode`), while Cache
  sections keep a library's state as it is (Rapier's joint motors hold `max_force: Real::INFINITY`).
- The reasons: NaN's bits differ between targets (section 2), and a NaN spreads through physics
  without a sound, so finding the tick it entered is worth more than the rare game that wants one.
- Library functions may return NaN or an infinity as an intermediate value (`asin(2)`, `exp(800)`);
  code handles it before writing.
- **Signed zero is kept.** `-0.0` arises from IEEE operations identically on every target once `min`
  and `max` are replaced (section 5), so it is stored and hashed as it is. Into an integer field, -0
  stores 0 (section 8). JavaScript's `JSON.stringify(-0)` is `"0"`, so a value that crosses JSON
  from a JavaScript client arrives as +0; the boundary write records what arrived.
- **Subnormals are kept** (no flush to zero, section 4).

## 8. Conversion into integer fields

A double entering an integer field (a script's write, a boundary write's JSON number, a Rust
conversion) is accepted only when it is:

1. finite, else refused as not finite;
2. integral (`x == x.trunc()`), else refused as not an integer: nothing rounds, truncates, saturates
   or wraps, so 2.5 computed for a `u32` is an error naming the field, the entity and the value,
   with the hint to use `Math.round`, `Math.floor` or `Math.trunc`;
3. within the field type's range (3.2), else refused as out of range, with the range.

-0 stores as 0. Nothing in the call is applied when any check fails (charter 3.4); for a script that
means the invocation fails as a whole (simulation.md, 4.5), the error naming the line
(`script-sandbox.md`, 5). The script host reports its own codes (`script.write_not_finite`,
`script.write_not_integer`, `script.write_out_of_range`); other callers use this specification's
(section 9).

```rust
pub enum NumError { NotFinite, NotInteger, OutOfRange { min: f64, max: f64 } }
pub fn to_i32(x: f64) -> Result<i32, NumError>;
pub fn to_u32(x: f64) -> Result<u32, NumError>;
pub fn to_tick(x: f64) -> Result<Tick, NumError>;
pub fn finite(x: f64) -> Result<f64, NumError>;
// EntityId::from_f64 is simulation.md, 7.1.
```

Integer arithmetic in Rust: the tick-code crates build with `overflow-checks = true` in the release
profile too (a per-package profile override), so a debug and a release build behave alike and an
overflow is an internal error (simulation.md, 4.6), never a silent wrap. Arithmetic that is meant to
wrap (hashes, PCG32) says so with `wrapping_*`. In scripts, integers are doubles, exact to 2^53;
`|0`, `>>>` and `Math.imul` are exactly specified by ECMAScript.

## 9. Error codes

| Code | When | `detail` |
|---|---|---|
| `number.not_finite` | a NaN or an infinity offered to a persisted field outside the script host | `{field, value}` (`value` as a string: "NaN", "Infinity") |
| `number.not_integer` | a non-integral value offered to an integer field | `{field, value, hint}` |
| `number.out_of_range` | an integral value outside the field type's range | `{field, value, min, max}` |
| `number.float_env_changed` | the control register is not the default at a tick's start (internal) | `{register, value, expected}` |

## 10. Checks and tests

1. **Golden sweep** (`math.golden`): the 26 sweep hashes of section 6.4 for `math`'s functions,
   committed; computed by the native debug and release builds and by the web build in headless
   Chrome (`checks.md`, 7.2), all equal to the file. A `libm` update that changes a result fails
   here first. The same file pins sweeps of the allowed operations that reach a C library in some
   build: Rust's `%` with subnormal, huge, negative and negative-zero operands, `floor`, `ceil`,
   `trunc`, `sqrt` and `math::min` and `max`, and `math::canonical` over NaNs of every sign and
   payload; script-host.md 12, test 2, sweeps the same operations in QuickJS-ng.
2. **Accuracy** (`math.accuracy`): a committed table of inputs and correctly rounded results per
   function, generated offline with mpmath (Python is offline tooling; the check reads the table and
   never runs Python); every function's error stays within 2 ulp, so an update that makes accuracy
   worse is caught even when it keeps determinism.
3. **Special values** (`math.special`): per function, the results for ±0, ±infinity, NaN, the
   smallest subnormal and the domain edges, as C99 Annex F gives them, and the ECMAScript cases of
   `Math.pow` and `Math.hypot` (6.3).
4. **Float environment** (`float.env`): a test that sets flush-to-zero on the game thread before a
   tick must trip `number.float_env_changed`.
5. **Writes** (`number.writes`): writes of NaN, ±infinity, 2.5 into `i32`, 2^31 into `i32`, -1 into
   `u32`, -0 into `u32` (stores 0) and 2^53 into `tick`, from a script and from a boundary write,
   give the right codes and apply nothing.
6. **Lint fixture** (simulation.md, 12, item 8): includes every entry of section 5.
7. **End to end**: the sailing scene's hash chain across debug and release builds and across the
   native and web builds (simulation.md, 12, item 10).
8. **parry's `log2`** (`physics.bvh_log2`): the expression of section 5 with `f32::log2` and
   `libm::log2f` gives equal rounded subtree counts for every `num_leaves` from 1 to 1,000,000,
   until the glamx and parry patch makes the test moot.

## 11. Open choices

1. **The solver's precision.** Settled by the physics spike: `f64` in every persisted component and
   the solver `f32` (`rapier3d`), the configuration it proved, with the conversion confined to the
   physics sync and write-back (3.1). `rapier3d-f64` was not measured.
2. **`libm` against an own library.** Recommendation: `libm` (6.1, measured). Because the charter
   says the library is "built from `+ - * /` and `sqrt`" and `libm` also uses rounding to integers,
   scaling, bit manipulation and correctly rounded fused multiply-add, the owner should accept the
   charter text "built from operations IEEE 754 rounds exactly" before slice 1 (AGENTS.md, rule 1,
   asks for the charter to change first). Proposed to the owner in [README.md](README.md), Proposed
   charter amendments.
3. **Replacing `Math` functions or removing them.** Recommendation: replacing, under their own
   names, as `script-sandbox.md` (2.2) does; removing them would make agents rewrite common code for
   no gain.
4. **Signed zero.** Recommendation: kept (7). Normalizing -0 to +0 on every float write costs a
   comparison per write and hides nothing that determinism needs.
5. **Strict integer conversion.** Recommendation: strict (8), as `script-host.md` (6) also
   recommends. Rounding on write would hide bugs that the structured error names at the line.
6. **Slice 1: the golden sweeps.** `pocket_sim::math::sweep` defines 36 sweeps of 100,000 inputs:
   the 26 of 6.4 (each function of 6.2 over its sampled range, `sin` also over ±1e5 and ±1e9) and
   ten of the allowed operations of 10, item 1 (`%`, `floor`, `ceil`, `trunc` and `sqrt` over finite
   doubles of every exponent with signed zeros, `min` and `max` over signed zeros and NaNs,
   `canonical` over NaNs of every sign and payload, `powi`, `wrap_angle`). Inputs come from PCG32
   seeded by the sweep's name, built by integer bit construction and exact arithmetic; each result's
   bits are folded into FNV-1a 64. The hashes are committed as
   `crates/pocket-sim/src/math/golden.txt` (rewritten only by
   `POCKET_BLESS=1 cargo test -p pocket-sim --test math golden_sweep`, with a `libm` change); native
   debug, native release and the `wasm32-unknown-unknown` build reproduce them (the web tests
   below).
7. **Slice 1: the accuracy table holds 120 inputs per function**, not 3,000: 3,000 lines over the 25
   functions and ranges of 6.2 (`crates/pocket-sim/tests/golden/accuracy.txt`, 120 KB, written by
   `accuracy.py` beside it with mpmath 1.3.0 at 200 bits), enough to catch an update that makes
   accuracy worse without a large committed file. Against the correctly rounded doubles every result
   is within 1 ulp (0 for `atan`, `exp2`, `log2`, `log10`, `cbrt` and `sqrt` on these inputs); the
   test's bound is 2.
8. **Slice 1: the web tests run in a module with no imports.** `pocket_sim::WEB_TESTS` lists
   `math.golden`, `rng.vectors` and `number.convert` as checks.md 7.2's registry asks; the example
   `web_vectors` builds them for `wasm32-unknown-unknown` as a module that imports nothing, and
   `crates/pocket-sim/tests/web/run.mjs` runs it under Node and compares its report byte for byte
   with the native one (`pocket_sim::web_report`, written by the `web_report` test). Node runs
   WebAssembly on V8, Chrome's engine; the same module also gave the native report in headless
   Chrome through `tools/webcheck.py` (2026-10-03). The check page's worker runs the same list once
   it exists.
9. **Slice 1: the float environment** is read at the start of every `Sim::step` (`stmxcsr` on
   `x86_64`, `mrs fpcr` on `aarch64`, nothing on `wasm32`); a mismatch poisons the world, and a unit
   test sets flush-to-zero on the test thread to prove it. Integer overflow checks are on in release
   and in `web` for `pocket-sim` through per-package profile overrides in the workspace `Cargo.toml`
   (8); each other tick-code crate adds its own.
10. **Slice 1: non-finite values in a `number.*` detail** are written as the strings `"NaN"`,
    `"Infinity"` and `"-Infinity"`, since JSON has no such numbers; integral doubles below 2^53 are
    written as JSON integers.

11. **Slice 1: the mass rule as `pocket-physics` builds it** (5). A convex hull, triangle mesh or
    compound collider takes no density on any body, fixed ones included, since Rapier recomputes
    every body's mass properties from its colliders whatever its kind (rapier3d 0.36.0
    `src/pipeline/user_changes.rs`), so a fixed island's hull with a density would put mass
    properties computed with the platform's math into the cache. A dynamic body takes its mass from
    exactly one place, its `RigidBody`'s stated mass properties or the density of a ball, cuboid or
    capsule collider: parry sums two non-zero mass properties through `with_inertia_matrix`, whose
    eigen decomposition calls the platform's math too (parry3d 0.31.1
    `src/mass_properties/mass_properties.rs`, `Add`), while a zero one is skipped; a dynamic body
    with no mass or with both is `sim.body_invalid`. A collider without a density gets Rapier's
    density 0, which skips the shape's mass computation entirely
    (`ColliderMassProps::mass_properties`), and stated mass properties enter as
    `MassProperties::new`, along the body's axes, with no eigen decomposition. One collider per
    body; a compound carries several shapes. `pocket_physics::validate` makes these refusals, for a
    boundary write to refuse a spawn at load; `physics.step` makes them again when it would build
    the body, reporting `sim.system_failed` with the cause each tick and building nothing.
12. **Slice 1: non-finite solver state** (7). A body that Rapier quarantines, or whose pose or
    velocity comes back non-finite, is reported (`number.not_finite`, the cause of a
    `sim.system_failed` of `physics.step`), keeps its `Transform`, and gets zero velocity both in
    the solver and in its `Velocity` component. Section 7 leaves the components "as they were", but
    `physics.step` syncs a `Velocity` written from outside into the solver, so a velocity left in
    the component would go back into the solver at the next tick.
13. **Slice 1: the forces in `f64`** (3.1). Wind, buoyancy, sail and hull forces are computed in
    `f64` with `math`, gathered per body in the component `ExternalForce` (force, and torque about
    the centre of mass) and rounded to `f32` once per body per tick in `physics.step`. A tick of the
    sailing scene (the Sloop's 74 buoyancy points, six crates of 27, four waves) took 41 us
    natively, the forces 26 and the step 14
    (`cargo run -p pocket-physics --example physics_bench --release`, R1 shared with other builds,
    2026-10-03); the spike's `f32` forces took 19. Most of it is buoyancy's 1,888 sine and cosine
    pairs a tick; one pair per wave and body with the angle-addition formulas for the points'
    offsets is the next step if `tick.sail` needs it.
14. **Slice 1: the cross-target check of physics** (10, items 7 and 8). `pocket_physics::WEB_TESTS`
    holds `physics.chain` (the sailing scene's hash at every tick of 1,200, folded into a chain that
    must equal `crates/pocket-physics/src/probe/golden.txt`), `physics.fork` (a fork at every tick
    of the first 600 continues 10 ticks with the original's hashes) and `physics.bvh_log2` (item 8).
    They pass natively in debug and release, and the example `web_physics`, a
    `wasm32-unknown-unknown` module with no imports, gives the native report byte for byte (the
    1,201 hash lines included) under Node (`crates/pocket-physics/tests/web/run.mjs`) and in a
    Chrome worker through `tools/webcheck.py` (`tests/web/index.html`), 2026-10-03. The hash is a
    stand-in, FNV-1a over the declared sections in bincode (`pocket_physics::probe`), because
    `pocket-physics` cannot link `pocket-persist`; the check command's `web` step compares the real
    `TickHash` chains.
15. **Slice 1: physics values are judged in `f32`** (3.1, 7). The components are `f64` and the
    solver `f32`, so a value `f64` holds can reach the solver as 0 or infinity: a ball of radius
    1e39 never moved, a ground of half extents 1e39 let a ball fall through it, a radius of 1e-50 or
    stated mass and inertia of 1e-300 made a body of zero mass, and a position of 1e39 came back as
    `number.not_finite` every tick, all with nothing refused. `pocket_physics::validate` and
    `physics.step` therefore judge each value as `f32` rounds it: sizes, masses, inertias and
    densities positive and normal there (at least 1.2e-38), everything else finite there (points,
    part positions, the mass centre, friction, restitution, damping, the gravity scale and a
    `Transform`'s position), refused as `sim.collider_invalid` or `sim.body_invalid`. A density in
    range can still give a mass or inertia that is 0 or infinite in Rapier's own `f32` formulas (a
    ball of radius 1e-30): `physics.step` reads the built collider's mass properties as Rapier
    computes them and refuses it (`sim.collider_invalid`); `validate`, which builds no Rapier shape,
    does not catch that case.

## 12. References

- Charter: `docs/charter.md` 3.3, 4.2.4, 4.2.7, 7.2.
- Master: `docs/design/networking.md` (Determinism), `engine/core/src/repro.cpp`,
  `sdk/runtime/repro.ts` (commit 50514a2d).
- Amoris predecessor charter (`origin/rebuild:docs/charter.md`, 8.2).
- Rust RFC 3514, "Float semantics"; Rust standard library documentation of `f64` (precision of the
  math functions, `min` and `max`).
- The `libm` crate 0.2.16 (rust-lang/compiler-builtins), a port of musl's libm.
- IEEE 754-2019; ISO C99 Annex F (special values); ECMAScript's `Math` (exact and
  implementation-approximated functions).
- WebAssembly core specification, floating-point NaN propagation.
- Microsoft, `D3DCREATE_FPU_PRESERVE` (Direct3D 9).
- mpmath 1.3.0 (reference values).
