# Depth prepass

Status: Draft, implemented on branch `explore/prepass` (2026-10-10)

Charter: 4.4 (the Pioneer note of 2026-10-10 on the depth prepass; the opaque pass, two-phase Hi-Z
and TAA notes of 2026-10-09), 4.1 (one renderer for Metal, Vulkan, Direct3D 12 and WebGPU).

This specification says how the opaque forward pass draws depth first and then shades only the
visible samples: the passes, why both compute the same depth, how it fits occlusion culling's two
phases, TAA and the three draw paths, the modes and the auto mode, and how it is checked. The code
is `crates/pocket-render/src/prepass.rs` (the modes, the auto mode), `renderer.rs` (`render`,
`DepthPrepass`, `forward_pair`, `depth_pipelines`) and `shaders/forward.wgsl` (`@invariant`,
`vs_depth`, `vs_depth_masked`, `fs_depth_masked`). Measurements:
[bench/prepass.md](../bench/prepass.md).

Why: on many_cubes' dense grid (1.6M cubes, about 482,000 in the frustum, 443 visible) with
occlusion culling off, the opaque pass shades in draw order and pays for every overdrawn sample:
Unreal Engine 5.7's forward renderer, which always draws a depth prepass, drew the same frame in a
third of our time on Direct3D 12 (branch `explore/ue5`, `docs/bench/ue5-dense.md`). Occlusion
culling (occlusion.md) removes hidden instances, not hidden samples: it does nothing when its auto
mode is off, on the first frame and after a disocclusion, and nothing for overdraw among visible
instances.

## 1. A frame

Without the prepass the opaque pass clears the color and depth targets and draws every variant's
batches with depth test `Greater` and depth writes (reversed-Z), then the ocean, the sky, particles
and the editor grid. With it (`Prepass::begin_frame` says so for the frame):

1. **Depth prepass** (`DepthPrepass::draw`, timestamp scope `depth prepass`): a render pass with
   the opaque pass's depth target only, cleared, every variant's batches of the camera's argument
   set through the depth-only pipelines, depth stored.
2. **Opaque pass**: the color targets cleared, the depth loaded, every variant's batches through
   the forward pipelines' equal-depth twins (depth test `Equal`, no depth writes), then the ocean,
   the sky, particles and the grid as before (they keep their own depth states).

With occlusion culling's two phases (occlusion.md 1) each phase gets its prepass: the early
prepass (cleared) and the early opaque pass draw last frame's visible set; the depth pyramid is
built from that depth; the late culling pass runs; the late prepass (loading the depth) and the
late opaque pass draw the newly visible instances, the late opaque pass then the sky and the rest.
The late culling pass overwrites the early records in `drawn` (occlusion.md 6), so the two phases
cannot share one opaque pass after both prepasses without doubling `drawn`; the early opaque pass
shades samples a late instance may cover, as it does without the prepass.

A frame without resolved, visible mesh instances never draws the prepass. Scene keeps an
incremental visible-slot count: removed, hidden or still-loading instances do not participate,
even when the GPU's slot scan range retains holes from earlier frames.

## 2. The same depth in both passes

The equal-depth test keeps a sample only if the forward pass computes exactly the depth the
prepass stored there. Both vertex stages compute the clip position with the same expression from
the same inputs: the instance's `Drawn` record (`drawn[instance_index + batch.x]`), the vertex
position from the shared mesh pool (a skinned part's own copy), `d.pos + quat_rotate(d.rot,
v.position * d.scale)`, then `view.view_proj * vec4f(world, 1.0)` with the same frame group (TAA's
jitter included). The position is `@invariant` in `vs` (`VsOut.clip`), `vs_depth` and
`vs_depth_masked` (`DepthOut.clip`), which requires every compiler to compute it identically in
each. naga writes it as:

| Backend | What naga writes |
|---|---|
| HLSL (Direct3D 12) | `precise float4 ... : SV_Position` in the output struct or return type |
| SPIR-V (Vulkan) | `OpDecorate %position Invariant` on the `Position` built-in |
| MSL (Metal) | `[[position, invariant]]` (MSL 2.1 or later; wgpu-hal compiles with `preserveInvariance`) |

`shaders.rs`' test translates the forward shader's three vertex stages with naga and requires each
form; without the attribute it fails at the first (`vs: no precise position in the HLSL`).
Without `@invariant` the depths still agreed on the RTX 5060 and the Radeon 780M with Direct3D 12
and Vulkan (tests/prepass.rs passes there too): the drivers compiled the same expression the same
way. The attribute is what guarantees it on other drivers and after other changes to the shaders;
it applies to the forward pass with or without the prepass, so the images with and without it
stay comparable.

The shadow casters (`vs_shadow`, `vs_shadow_masked`) and the entity-id pass keep their outputs
without the attribute: they are not tested for equality.

## 3. Pipelines

- **Depth only** (`depth_pipelines`, by forward variant): the forward variant's culling (back
  faces culled, or none for double-sided), `vs_depth`, or for alpha-masked variants
  `vs_depth_masked` and `fs_depth_masked`, the alpha test of `fs_masked` (the base color's alpha
  times its texture's, sampled with the uv's gradients, against the material's cutoff). Depth test
  `Greater`, written. The one-sample set (`depth_only`) also draws the unjittered depth splats test
  against with TAA (taa-gtao.md); a multisampled opaque pass's prepass uses a set built for its
  samples (`depth_msaa`, rebuilt when the samples change).
- **Equal depth** (`Forward::equal`, beside `Forward::test`): the forward pipelines with depth test
  `Equal` and depth writes off. Alpha-masked variants retain `fs_masked` and repeat the same
  alpha test as the prepass. Depth alone does not identify the surface that wrote it: another
  coplanar surface can write matching depth inside a mask's holes. Shading those fragments
  without their alpha test would fill fully transparent holes. The neural variants' twins use
  `fs_neural`.
- Variants: a neural variant (`NEURAL_VARIANT + v`) draws its depth through base variant `v`'s
  depth-only pipeline; the neural variants are skipped in the prepass exactly when the opaque pass
  skips them (before their pipelines exist), so no depth is drawn without its color. Levels of
  detail are mesh rows and skinned parts are meshes of their own (lod.md, occlusion.md 5): the
  prepass draws the batches the opaque pass draws, from the same arguments.
- Draw paths (batches.rs): the prepass calls `Batches::draw` for the same argument set and
  variants as the opaque pass, so the multi-draw, first-instance and baseline paths draw it as they
  draw the opaque pass (the baseline path's bases in the dynamic-offset uniform, group 3).
- Group layout: the depth-only pipelines use the shadow layout (frame, empty, textures, batch); the
  prepass binds the jittered frame group, the opaque pass's.

Each forward pair (two threads at start-up, par.rs) builds its two test pipelines and then their
two equal-depth twins, whose shaders wgpu has compiled by then (Direct3D 12 caches DXC's output
per device); each twin retains its pair's fragment entry point.

## 4. Fit with the rest of the frame

- **Occlusion culling**: the pyramid reads the depth after the early phase, which is the early
  prepass's depth (the early opaque pass does not write depth), equal to what the early opaque pass
  wrote without the prepass. The late test, the visibility state and the counters are unchanged, so
  no entity's coverage changes: tests/occlusion.rs runs its comparison with the prepass off and
  on, tests/hiz.rs passes with it on, and tools/occlusion_compare.py with the prepass on finds what
  it finds without it (identical coverage everywhere; one tied pixel in the sphere, occlusion.md 5).
  The occlusion test pins the prepass: in the auto mode its two renderers (one opaque phase, two)
  decide apart, and in one step of its sweep a sample where two surfaces tie in depth then showed
  the other surface (one pixel).
- **TAA**: both passes draw with the jittered frame group, so they agree under every jitter; the
  object motion and the other extra targets are written by the opaque pass alone. The unjittered
  depth for splats (taa-gtao.md) is drawn as before, its own pass.
- **Depth readers**: GTAO, TAA's disocclusion test and the splats read the depth after the opaque
  pass; it holds the same values as without the prepass (the ocean writes its depth in the opaque
  pass as before).
- **MSAA**: the depth test runs per sample, so the forward pass shades a pixel's covered samples
  whose depth matches and the resolve is unchanged.
- **Ties**: two surfaces at exactly the same depth at a sample both pass the equal test, so the one
  drawn last shows; without the prepass the first drawn one shows. This applies only to valid
  fragments: masked fragments below the alpha cutoff are discarded in both modes. The
  occluder demo's cylinder bottom cap and ground are also coplanar (both y=0), so they can
  show different winners even with the alpha test retained. Image-invariance fixtures lift
  that cylinder by 0.01, while a separate regression checks both valid tie winners and
  transparent holes with each mesh order. The demo itself remains unchanged. The many_cubes
  sphere also has tied samples (section 6), as when occlusion culling changes its draw order
  (occlusion.md 5).

## 5. Modes

`PrepassMode` (`prepass.rs`): `Off`, `On` (every frame with instances) and `Auto`, the default.
`POCKET_PREPASS=off|on|auto` sets the starting mode natively, `Renderer::set_prepass` at any time,
the viewport page `?prepass=off|on|auto`. `FrameStats::prepass` says what the frame did (`off`,
`on`, `auto-on`, `auto-off`); the windowed benchmark reports it and the frames that drew it.

**Auto** decides by measurement, not a model: the opaque passes' GPU time with and without the
prepass, from the pass timestamps (profiler.rs; `Reading` sums the scopes `depth prepass`,
`opaque early` and `opaque+sky`, the passes that differ between the two ways; culling, the pyramid
and post are the same either way). Readings arrive two or three frames late natively (up to a second
in a browser), and the profiler does not time a frame whose readback buffer is still busy, which
`GpuProfiler::times_frame` says as the frame begins. Each reading names the frame it measured
(`Timings::frame`, the profiler's count of submitted frames; `GpuProfiler::arrived` keeps every
reading that arrived, oldest first), and a probe counts only its own frames: frames of its kind with
instances, timed, begun since it started. A frame without instances (a loading screen's sky alone,
0.1 ms), a frame from before a resize and a frame between probes never decide.

A probe first draws the way frames draw now until it has four readings of it (`SAMPLES`; in a
re-probe this costs nothing), at the first probe the prepass: its extra cost is bounded (a second
geometry pass) where shading in draw order has no bound. Then it draws the other way one frame at a
time: only in a frame the profiler times, and only once the last such frame's reading has arrived
(or a later frame's, which shows it lost), at most eight frames a probe (`OTHER_FRAMES`). The
incumbent stays unless the other way is cheaper by 5% (`MARGIN`); the first probe's incumbent is no
prepass, which draws less. The minimums decide (the GPU's interference only adds time), and more
readings of the way drawn second can only lower its minimum, so it wins as soon as the minimums say
so; it loses after four readings, or at its first reading more than 50% above the first way's
minimum (`BAIL`). A probe therefore draws a much slower way for one frame (on the dense grid, one
frame without the prepass at three times the GPU time), and a way within 50% for at most four
frames, never two in a row with a readback lag of two frames or more.

The next probe comes 240 frames later (`PROBE_EVERY`) after a decision that changed the choice, and
twice as long after each one that kept it, up to 1,920 frames (`PROBE_MAX`). A probe still waiting
600 frames of any kind after it started gives up (`PROBE_TIMEOUT`: the choice stays, a later probe
is scheduled), so every reading a probe takes measured a frame within that span.

Frames with one opaque phase and frames with two (occlusion culling, occlusion.md 7) are timed and
decided separately (`Choice` per kind): occlusion culling changes what the opaque passes draw, so a
reading of one kind says nothing about the other, and occlusion culling's own auto mode switches
between them. Where two-phase frames come only with occlusion culling's probes (two frames every 120
to 960 frames), a prepass probe of that kind may not finish within 600 frames; it gives up and the
kind keeps its choice. A resize or a change of the opaque pass's format (anti-aliasing, GTAO) drops
the running probes and probes again at the next frame of each kind, which draws as decided before
until its probe decides. Without timestamps (WebGPU without `timestamp-query`, `POCKET_GPU_MINIMAL`
with `timestamps`) the auto mode never draws the prepass; `On` still does.

The default mode is auto, chosen by measurement (bench/prepass.md 3): neither fixed mode is best
everywhere. On many_cubes' dense grid the prepass cuts the GPU time to a third without occlusion
culling and by a third with it (the early phase's visible cubes overlap); on the sphere, where the
cubes hardly overlap, it costs 19 to 35% more, a second geometry pass for little. Auto draws it in
the first and not in the second, at the cost of its probes every 240 to 1,920 frames: one frame of a
much slower way, or up to four of a way within 50% (bench/prepass.md 3 gives their frame times).

## 6. Checks

- `crates/pocket-render/tests/prepass.rs` draws, with two renderers on one device (the prepass off
  and forced on) through the same frames: the mixed scene (every variant in the camera and four
  shadow cascades) with three skinned characters, with 4x MSAA; the occluder scene with its
  cylinder lifted off the ground and occlusion culling forced on (cold, warm, a camera cut);
  the anti-aliasing scene with TAA and with MSAA+TAA,
  its instances moving and its camera panning (frames 1, 5 and 12); the LOD field; a constant
  neural material on a ground under four primitives. Every frame's pixels must be identical and
  every entity's id-pass coverage equal; the mixed and occluder scenes run again on the two other
  draw paths. A prepass vertex stage computing the same position in another order
  (`view_proj * pos + view_proj * rotated`) makes 20,695 pixels of the mixed scene differ (16%),
  and the test fails. It passes on the RTX 5060 and the Radeon 780M with Direct3D 12 and Vulkan, and
  with `POCKET_GPU_MINIMAL=features,limits,timestamps`.
- The coplanar regression draws a fully transparent masked quad over a blue quad at the exact
  same depth. Every pixel must match the blue quad drawn alone, with prepass off and on, single-
  and double-sided masks, one sample and 4x MSAA, opaque and masked blue materials, and both
  mesh orders. When both materials are masked, reversing mesh rows also reverses actual draw
  order. A fully opaque alpha texture then checks the documented tie rule: the first drawn
  surface wins without the prepass and the last wins with it. These are valid surfaces, unlike
  the transparent fragments that must never win a depth tie.
- `shaders.rs`' test checks what naga writes for the invariant positions (section 2).
- `prepass.rs`' unit tests drive the auto mode through a simulated renderer and profiler (a readback
  lag, the ring of three readback buffers, lost readbacks, frames without instances): the readings;
  the decisions where the prepass pays and where it does not, within and beyond the margin; empty
  frames at the start (a loading screen) and between probes, which never decide; a resize between
  and during probes, whose old frames' readings arrive after it and never count; the timeout counted
  in frames of any kind; one frame of a much slower way per probe, either way round; at most four
  frames of a way within 50%, one at a time; lost readbacks, irregular and in step with the probe
  (at most eight frames the other way); a browser's readback lag; the re-probe after a change, the
  backoff, separate one- and two-phase choices, no timestamps. With a defect put back each of these
  fails: readings counted whatever frame they measured (the review's empty-frame and resize cases),
  the timeout counted in frames of the kind, both ways drawn by turns as before (runs of frames
  without the prepass at each re-probe of the dense grid), no bail-out, several frames of the other
  way in flight, no cap on them.
- `cargo run --release -p pocket-app --example draw_paths -- --compare prepass <scenes>` draws a
  scene twice at one moment (off, forced on) and compares pixels and coverage;
  `python tools/prepass_compare.py` runs that over the samples and every capture of
  tools/backend_compare.py with `POCKET_PREPASS=off` and `on`. Results in bench/prepass.md 2.

## 7. Limits and open questions

- The prepass draws every triangle twice: its cost is a second geometry pass (vertex work and
  rasterization) and the depth target's store and load between the passes (at 2560x1440 with 4x
  MSAA, 59 MB each way before compression). It pays where many samples are shaded more than once.
- The early phase of occlusion culling shades with its own prepass; a late instance in front of an
  early one still costs the early one's shading there (section 1).
- The auto mode needs timestamps; without them (some browsers) it stays off. An estimate from
  counters (the culling pass's projected area against the screen's) could decide there; not done.
- Two surfaces tied in depth resolve to the last drawn (section 4).
- Shadow cascades have no prepass (depth only already).
