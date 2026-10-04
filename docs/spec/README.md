# Specifications

- Status: Draft, slice 0, integrated 2026-10-03. Every decision below is Proposed until the owner
  accepts it.
- Charter: 7 (the specifications slice 0 writes), 3.10 (performance benchmarks, measured and not
  gated), 10 (slice 0 and its acceptance), 12 (open questions).

This file indexes the slice 0 specifications, says which charter item each covers, records the
verdict of each slice 0 spike, lists the decisions slice 0 proposes for the charter's open questions
and the charter amendments they need, and records the contradictions the integration step resolved
between specifications drafted in parallel. The spike reports are under [`../spikes/`](../spikes/);
their code is under `spikes/`.

## 1. Index

| File | Owner | Covers |
|---|---|---|
| [simulation.md](simulation.md) | spec-sim | Charter 7.1 (tick semantics: fixed timestep, boundaries, phases, system order, events), 7.4 (entity iteration order); entity identity and `Name`; the hooks the time modes use (3.5) |
| [numeric.md](numeric.md) | spec-sim | 7.2 (which state is floating point, the deterministic math library, integer conversion); the rules that keep Rapier's own math deterministic |
| [rng.md](rng.md) | spec-sim | 7.3 (PCG32, seed derivation, streams per system and entity) |
| [persistence.md](persistence.md) | spec-persist | 7.5 (canonical serialization), 7.6 (world hash); snapshot, restore and fork (3.3) |
| [replay.md](replay.md) | spec-persist | 7.5 (the replay format), 3.3 (replay and the first divergence); recording, segments after a restore or reset, seeking, `verify`, `lockstep` |
| [versions.md](versions.md) | spec-persist | 7.13 (engine and bundle versions, schema versions and migrations, saves) |
| [script-host.md](script-host.md) | spec-script | 7.7 (script host API, batch queries, executors, `.d.ts`, the Rust API, tests, debugging); 4.2 |
| [script-sandbox.md](script-sandbox.md) | spec-script | 7.7 (the freeze and the lint, the execution budget, call depth and faults, the QuickJS-ng patches, script errors); 4.2.4, 4.2.5 |
| [hot-update.md](hot-update.md) | spec-script | 7.14 (hot update at a boundary, kept on failure, tried in a fork) |
| [debugger.md](debugger.md) | slice 1 | Charter 4.3 (the script debugger: its core, the CDP endpoint, the agents' `debug.*`, P9 and P10, its cost and its checks against VS Code's js-debug and Chrome DevTools) |
| [threads.md](threads.md) | spec-arch | 7.15 (the game and editor threads, publication, the queue and its order, the web form); 5.1 |
| [architecture.md](architecture.md) | spec-arch | 3.9 (crates with one-way dependencies), 4.1 (stack and the `wasm32` build), the build rules |
| [checks.md](checks.md) | spec-arch | 3.7 (the local check command), 3.9 (the crate boundaries it verifies; no line limit), 9.4 (the dependency check per commit) |
| [host-protocol.md](host-protocol.md) | host | Charter 3.1, 4.3, 4.5 (the host's endpoints, requests, events and methods shared by the editor, agents and tools) |
| [editor.md](editor.md) | editor | Charter 4.5 (the web editor: panels, shortcuts, the viewport renderer interface, the mock host, what the editor needs from the host) |
| [budgets.md](budgets.md) | spec-arch | 3.10 and 12 item 2 (benchmarks on reference hardware and their reference figures, reported and never a pass condition), with the figures slice 0 measured |
| [shared/contract/README.md](../../shared/contract/README.md) | spec-contract | 6.1 and 6.2 (the shared contract, its versioning and sync), seats and roles, the game definition, conformance checks |
| [shared/contract/perception.md](../../shared/contract/perception.md) | spec-contract | 7.8 (perception API); 3.1 |
| [shared/contract/projection.md](../../shared/contract/projection.md) | spec-contract | 7.8 (the text, JSON and tensor projections, rounding); 3.1 |
| [shared/contract/sailing.md](../../shared/contract/sailing.md) | spec-contract | 2.4.1 (the sailing showcase's perception, controls, intents, pacing and codes) |
| [shared/contract/actions.md](../../shared/contract/actions.md) | spec-contract | 7.9 (controls, intents, affordances); 3.4 |
| [shared/contract/time.md](../../shared/contract/time.md) | spec-contract | 7.10 (turns, lockstep, pausable real time, stepping, pause-on-decision, halts); 3.5 |
| [shared/contract/errors.md](../../shared/contract/errors.md) | spec-contract | 7.12 (`{code, message, detail}`, refusal with suggestions) |
| [shared/contract/mcp.md](../../shared/contract/mcp.md) | spec-mcp | 7.11 (player-facing and developer-facing MCP tools) |
| [shared/benchmark/README.md](../../shared/benchmark/README.md) | spec-mcp | 9 items 1 and 2 (the harness, metrics, statistics), 10 (the slice 0 skeleton) |
| [shared/benchmark/tasks.md](../../shared/benchmark/tasks.md) | spec-mcp | 9 (developer and player tasks, fixtures, reference policies) |

The shared contract and benchmark are a first draft of PocketEngine's `shared/` (charter 6.2):
PocketEngine's `rebuild` is still at 5f8971e, which has no `shared/`, so nothing is synchronized yet
and `shared/SYNC.toml` records `commit = ""` with each shared file's hash (checks.md 5.4, where
`contract.sync` is `Skipped` until a commit is recorded).

## 2. Slice 0 verdicts

Each spike was built by one agent and reproduced by another (the verification), on the reference
laptop R1 (`budgets.md`, 3) while other agents' builds shared it.

| Spike | Charter check | Verdict | Why | Report |
|---|---|---|---|---|
| script-native | 4.2.7 rule-workload time per tick; the determinism setup of 4.2.4 | Works with caveats | 1,000 entities through the batch API in 2.4 to 3.3 ms a tick at the median, bit-identical to the same rule in Rust; the caveats are the interpreter's speed (21 to 27 times Rust) and four holes its verification found (`**` through `eval`, lost promise-job errors, panics across QuickJS-ng's C frames, unbounded host allocation), which script-host.md closes | [script-native.md](../spikes/script-native.md) |
| script-web | 4.2.7 the `rquickjs` `wasm32` build | Works with caveats | QuickJS-ng builds for `wasm32-unknown-unknown` with `wasm-bindgen` and matched the native hash at 600 of 600 ticks in a Chrome worker, with and without cross-origin isolation; caveats: a Windows include-path workaround, and a poll count that depends on history | [script-web.md](../spikes/script-web.md) |
| physics | 4.3 physics and buoyancy determinism | Works with caveats | Rapier 0.36.0 with buoyancy, sail and hull forces matched natively and in Chrome at 3,001 of 3,001 ticks, and forks at every tick continued identically; caveat: Rapier's own code calls the platform's math natively for some mass properties, which numeric.md 5 rules out of games until patched | [physics.md](../spikes/physics.md) |
| render | 4.4 high-poly glTF import | Works with caveats | A 48.2M-triangle scene imports off the main thread in 3.3 s and draws in 0.18 ms of GPU time (p95) with levels of detail on the RTX 5060; caveats: offscreen only, no sea, shadows or textures, and unexplained 61 to 123 ms frame gaps in Chrome | [render.md](../spikes/render.md) |
| threads | 5.1 the two-thread model natively and on the web | Works | Every property held at 10^2 to 10^5 entities natively and in a worker, with and without isolation: a stalled or paused game never froze the editor, a stalled editor never moved a tick, every session replayed natively; on Windows the loop must wait in 1 ms slices or under `timeBeginPeriod(1)` | [threads.md](../spikes/threads.md) |
| debugger | 4.2.6 PR #1421 breakpoints through `rquickjs` in VS Code | Works with caveats; the VS Code check is open | The PR applies unmodified to QuickJS-ng 0.16.2 and a CDP client passed 19 of 19 checks presenting TypeScript; no real VS Code or Chrome DevTools session was run, and the installed handler doubles statement cost | [debugger.md](../spikes/debugger.md) |

## 3. Slice 0 acceptance

Charter 10 (as amended in 0.4): "Specifications settled and first performance measurements recorded;
every check has a verdict."

| Item | State |
|---|---|
| The core specifications and the shared contract | Drafted and integrated (section 1); Draft until the owner accepts them |
| Performance measurements | Partly recorded: frame time (`frame.gpu`, `frame.cpu`, `web.frame`), the sail tick (`tick.sail`) and fork time (`fork.sail` and `branch.sail`, which rest on proxies or nothing) are unmeasured, with most rows of `budgets.md` 5 "not measured"; reference figures are set where a spike measured them, and slice 1's first calibrated run records the rest with `cargo xtask perf --set-unset` (`budgets.md` 8.1); findings in `budgets.md`, 13 |
| The two-thread model, native and web | Works (threads) |
| Physics and buoyancy determinism | Works with caveats (physics) |
| PR #1421 breakpoints through `rquickjs` in VS Code | No verdict: the patch and a CDP endpoint work, VS Code itself was not run; script-host.md 13 gives the headless run through VS Code's own debug adapter that will give one |
| The `rquickjs` `wasm32` build | Works with caveats (script-web) |
| Rule-workload time per tick | Measured: `tick.rules`, reference figure 4.0 ms (script-native) |
| High-poly glTF import | Works with caveats (render) |
| Benchmark harness skeleton | Specified (shared/benchmark/README.md, 8), not built |
| The local check command, which reports performance measurements | Specified (checks.md), not built |

Four items therefore still lack a verdict: the performance measurements, the VS Code session, the
harness skeleton and the check command.

## 4. Decisions proposed for the charter's open questions

All Proposed; the owner accepts or changes each.

1. **Physics (12.1).** Proposed: Rapier, `rapier3d =0.36.0` with `enhanced-determinism` and
   `serde-serialize`, never `parallel` or `simd8`, an `f32` solver with `f64` components, every
   engine force through the `libm`-based math library, and, until glamx and parry are patched,
   explicit mass properties on dynamic hulls, trimeshes and compounds and no joints or CCD in games
   (numeric.md 5; architecture.md 4.4; persistence.md 8). Box2D v3 and Jolt were read, not built:
   their bindings offer no serializable world or no `wasm32-unknown-unknown` route
   (`docs/spikes/physics.md`, Alternatives).
2. **Reference scenes and hardware (12.2).** Proposed: R1, the owner's ASUS ROG Zephyrus G14 (Ryzen
   9 270, RTX 5060 Laptop natively, Radeon 780M for the web in headless Chrome), with the workloads
   of `budgets.md` 4 and the reference figures of `budgets.md` 5. Most figures are not measured yet
   and are set by slice 1's first calibrated `perf` run.
3. **Large high-poly assets (12.3).** Proposed: external storage pinned by content hash, as master
   does since the owner's decision of 2026-10-02 (master AGENTS.md, rule 12; its `tests/data.json`),
   which matches how a world names assets by `ContentHash` (architecture.md 4.2). The render spike's
   122 MB scene was generated by a script instead; generated assets need no storage, but its
   generator uses the platform's `sin`, so its bytes are guaranteed only on one OS.
4. **The comparison deadline (12.4).** No proposal: slice 0 produced no evidence on it.
5. **The PR #1421 debugger patch (12.5).** Proposed: maintain it, as the unmodified upstream diff
   applied by `cargo xtask vendor` to a pinned `rquickjs-sys` committed under `third_party/` and
   substituted through `[patch.crates-io]` (architecture.md 7.5), together with the script host's
   patches P1 to P7 (script-sandbox.md 4.2); instrument scripts only while a debugger is attached;
   propose upstream, with the owner's consent, the `OP_debug` operand form (1.08 times instead of
   2.04 when installed) and the opcode-placement defect the spike found. The charter's VS Code check
   stays open until a session runs, headless through VS Code's own debug adapter (script-host.md
   13).
6. **WebAssembly for player-submitted code (12.6).** No proposal: outside slice 0's evidence.
7. **Sandboxing agents (12.7).** Proposed: each agent's own sandbox, confined to the task directory
   (shared/benchmark/README.md, 3.7); an operating-system boundary needs administrator rights and
   stays the owner's call.
8. **The web form of the two threads (12.8).** Proposed: a Web Worker with message passing as the
   only form: transferred buffers with at most two snapshots in flight, no `SharedArrayBuffer`, so
   it works without cross-origin isolation (`threads.md`, 7). The threads spike measured a
   `SharedArrayBuffer` triple buffer no cheaper to publish (0.185 against 0.160 ms at 100,000
   entities).

## 5. Proposed charter amendments

AGENTS.md rule 1 asks for the charter to change before a specification departs from it.

1. **4.2.4 and 7.2, the math library.** The charter says "built from `+ - * /` and `sqrt`". The
   specifications use the `libm` crate, which also uses rounding to integers, scaling by powers of
   two, bit operations and correctly rounded fused multiply-add, each exact under IEEE 754; it gave
   identical bits natively in debug and release and in Chrome over 26 sweeps, and is more accurate
   than master's `repro` (numeric.md 6.1). Proposed text: "built from operations IEEE 754 rounds
   exactly".
2. **4.1 and 4.3, physics.** The table's "Physics: Open" becomes Rapier, as section 4, item 1
   proposes.
3. **12, open questions.** Items 1, 2, 3, 5, 7 and 8 close as section 4 proposes once accepted.

## 6. Contradictions resolved

The specifications were drafted in parallel, without the spike results. The integration step made
each concept have one name and one owner's definition, and aligned the specifications with the
spikes:

1. **The error type.** `PocketError` (simulation.md, persistence.md, versions.md) and
   `ContractError` (an mcp.md draft) became `Problem`, spec-contract's, declared in
   `pocket-contract`.
2. **Event identity.** `EventId {tick, index}` (threads.md, perception.md, the contract README)
   became spec-sim's `EventSeq`, a world-wide sequence number.
3. **Events and the hash.** persistence.md said events were not world state and hashed an events
   digest beside the world; simulation.md double-buffers events and persists the inbox. The inbox
   and counter are Resources, the hash after a tick covers that tick's events, and `TickHash` is the
   world hash (persistence.md 2 and 5.3).
4. **Boundaries.** "The boundary before tick n" (simulation.md), "boundary n" (threads.md) and
   "B(t)" (persistence.md) became boundary n, after tick n; a point inside one is `(tick, writes)`,
   not `(tick, revision)` (simulation.md 2).
5. **Recorded writes.** `Applied { boundary, index }` (simulation.md) and `before_tick` (actions.md,
   errors.md, mcp.md) became threads.md's `Applied { tick, index }`, the tick a write is an input
   of.
6. **Two `Source` enums.** threads.md's command source keeps the name; actions.md's agent, human,
   script or peer became `ActOrigin`. Players are ordered by their seat's index, not a `PlayerId`.
7. **The loop's status.** simulation.md referred to a `Phase` that threads.md had named `LoopState`;
   spec-sim's tick stage is `TickPhase`.
8. **System slots.** The contract needed `interface.perception` and `interface.turns` in the
   `Finish` phase, which simulation.md lacked; they are rows 10 and 11. Perception read only "the
   tick's event record" and would have missed events that boundary writes append (an intent's
   start), while threads.md said boundary writes emit none: perception now reads the events appended
   since its last run, the inbox marks where the boundary's begin (`boundary_from`), and threads.md
   streams both.
9. **`Name`.** The contract and mcp.md assumed a `Name` component spec-sim had not defined;
   simulation.md 7.1 defines it.
10. **Halts after a script failure.** script-host.md recommended a pause to the contract, time.md
    called the policy spec-arch's, mcp.md spec-script's. time.md owns it (Halts), with spec-script's
    reason.
11. **Restart.** hot-update.md gave restart to the contract and mcp.md, mcp.md gave it to
    spec-script; it is mcp.md's `reset` and `apply {restart: true}`.
12. **Type errors in `apply`.** hot-update.md defaulted to reporting them, mcp.md refused on them;
    both take `types`, default `"report"`.
13. **Overflow checks.** numeric.md required them in the tick-code crates' release builds, checks.md
    left them open; numeric.md's rule stands.
14. **Rapier's math.** numeric.md said `enhanced-determinism` routes Rapier's math through `libm`;
    the physics verification found glamx, parry and Rapier calling the platform's math natively. The
    rules of numeric.md 5 and a cross-target fixture with a hull, a joint and CCD replace the claim;
    the fixture is a negative control until glamx and parry are patched (checks.md 8.6).
15. **Fork by cloning.** persistence.md said Rapier's world cannot be cloned; the physics spike
    cloned its parts in 5.3 to 5.7 µs. Forks go through bytes in slice 1, cloning measured as the
    alternative.
16. **`wasm-opt -O3`.** architecture.md shipped it on the script-web spike's evidence (no change in
    speed); the physics spike found Rapier's step slower and a 5 ms tick. The web check builds both,
    and the measurements decide.
17. **The math library.** The script-native spike bound a Rust port of master's `repro`, the
    script-web spike proposed a JavaScript library and a lint ban on `**`, spec-sim chose `libm`.
    One Rust library (`libm`) is installed as `Math.*`, `**` is lowered to `Math.pow`, and `eval`
    and the `Function` constructors are removed, since code they compile bypassed both.
18. **Steps and the hash.** The script-web recipe hashed the interrupt-poll count; its verification
    showed the count depends on what ran before. Steps never enter the world hash, and the counter
    is set at every call (P1), with the script-native spike's phase sync as the fallback.
19. **Web flow control.** threads.md allowed one snapshot in flight; the spike measured two. Two.
20. **"The edit applies first, always."** It holds within a batch only, as the threads spike showed;
    threads.md says so.
21. **When commands apply while running.** simulation.md held them for the next boundary; threads.md
    and the spike apply them on arrival (1.0 to 1.6 ms against 8.7 to 9.5 ms). On arrival.
22. **The snapshot slot.** threads.md recommended a mutex; the spike recommended `arc-swap`.
    `arc-swap`, for its lock-free reads.
23. **Error families.** errors.md lacked `version`, `migrate`, `scripts`, `types` and the check
    command's families, and mapped draft spellings no specification still uses; its table is
    complete now.
24. **File references.** Links to `contract.md`, a spec-mcp file under `docs/spec/` and unlinked
    spec names were replaced by the files' real paths.
25. **The reference rule workload.** script-host.md cited the debugger spike's 321 µs workload as
    the charter's; the script-native spike's 2.4 to 3.3 ms workload is, and budgets.md's warning
    based on the script-web spike's heavier workload is gone.
26. **`describe`.** The contract used it for the game definition and for an entity; it is one tool
    taking `entity` or `part`.
27. **rng.md's cross-reference** to simulation.md's open choices pointed at the wrong item.
28. **Rapier's features in the check.** checks.md required only `enhanced-determinism`;
    architecture.md and the physics spike need `serde-serialize` too, which the `deps` step now
    requires.

## 7. Open for slice 1 and the owner

- **Unmeasured budgets.** `budgets.md` 10 lists every reference figure no spike measured; the check
  reports them with the warning `perf.budget_unset` until slice 1's first calibrated run.
- **Checks without a verdict.** The performance measurements, the VS Code debugging session (planned
  headless, script-host.md 13), the harness skeleton and the check command (section 3).
- **Physics.** aarch64 and browsers other than Chrome untested; joints, CCD, sleeping and hundreds
  of bodies unmeasured; a patch routing glamx's and parry's math (parry's per-step `log2` included)
  through `libm`, which the physics fixture's negative control will prove (numeric.md 5).
- **Script host.** P1 to P7 not yet written, among them P5's call-depth limit, whose depth test sets
  the native stack sizes (256 KiB allowed only 50 nested calls with MSVC's QuickJS-ng;
  script-sandbox.md 4.3); the lint's alias gap; a TypeScript workload transpiled by oxc not yet run
  on the web.
- **Rendering.** A full frame with the sea, shadows and post-processing; the Chrome frame gaps; a
  presenter on a real surface.
- **The contract.** No implementation or conformance case exists yet; the skipper profile's numbers
  and the thinking clocks wait for the player benchmark.
