# The WebGPU baseline

Status: Draft, implemented on branch `explore/webgpu` (2026-10-09)

Charter: 4.4 ("browser WebGPU and native Metal/Vulkan run the same code; only features WebGPU offers
by default are the baseline, native optional features such as multi-draw indirect and timestamp
queries are acceleration paths"), 4.1 (one renderer for Metal, Vulkan and WebGPU), 3 item 7
(GPU-driven).

This specification says which optional GPU features the renderer uses, what it does without each,
how its GPU-driven indirect draws work on WebGPU's defaults, how a native run emulates a browser,
and how the paths are checked. The code is `crates/pocket-render/src/gpu.rs` (capabilities and the
switches), `crates/pocket-render/src/batches.rs` (the indirect draws) and the vertex stages of
`crates/pocket-render/shaders/forward.wgsl`. Measurements: [docs/bench/web.md](../bench/web.md);
evidence: `docs/evidence/webgpu/`.

## 1. Feature tiers

`Gpu::new` requests every feature in the table that the adapter offers and records the result in
`Gpu::caps` (`Capabilities`).

| Capability | Source | Used for | Without it |
|---|---|---|---|
| `indirect_first_instance` | WebGPU `indirect-first-instance` | a batch's instance base in its indirect arguments (2) | the base goes through a dynamic-offset uniform (2.3); identical images |
| `multi_draw_indirect` | wgpu-core on Vulkan, Metal, DX12 (`DownlevelFlags::INDIRECT_EXECUTION`); false in the browser | one `multi_draw_indexed_indirect` per view and variant | one `draw_indexed_indirect` per batch that holds instances |
| `timestamps` | `timestamp-query` (natively also inside passes and encoders) | per-pass GPU times (`profiler.rs`) | GPU times read 0 |
| `shader_f16`, `float32_filterable` | `shader-f16`, `float32-filterable` | requested when offered; nothing in the raster renderer reads the flags (gi/ checks the adapter itself) | — |

wgpu 30 does not tie `multi_draw_indexed_indirect` to a feature: it needs only indirect execution,
and its browser backend implements it as a loop of single draws because WebGPU has no multi-draw
(Chrome 155 exposes `chromium-experimental-multi-draw-indirect` behind `--enable-unsafe-webgpu`,
which wgpu 30 does not use). The two capabilities are therefore separate: first-instance support
decides where the base travels, native multi-draw decides how many calls the CPU makes.

## 2. Indirect draws

### 2.1 Batches

The culling pass (`cull.wgsl`) tests every instance against five views (the camera and four shadow
cascades). A batch is one (view, pipeline variant, mesh); the variants are opaque, alpha-masked,
double-sided and both (`scene.rs`, `VARIANTS`). The culling pass counts a batch's visible instances
into its indexed indirect arguments and writes each one to the batch's region of the view's list:
the camera view's interpolated poses to `drawn`, the cascades' instance indices to `visible`.
Instance `k` of a batch is entry `base + k`, with

    base = view * stride + offsets[variant * meshes + mesh]

where `offsets` are prefix sums of the instance counts per batch (`Scene::batch_offsets`) and
`stride` a view's entries. The vertex stages `vs`, `vs_shadow`, `vs_shadow_masked` and `vs_id` read
entry `instance_index + batch.x`; the shadow stages also derive the cascade as
`(instance_index + batch.x) / stride - 1`.

`Batches` (batches.rs) keeps the arguments' template (one set per batch, instance counts 0), copies
it over the live arguments at the start of each frame, and issues the draws. It is rebuilt when the
meshes or the per-batch instance counts change, never per instance.

### 2.2 The paths

`DrawPath::for_caps` picks one of three:

| Path | When | Arguments' `first_instance` | `batch.x` | Calls per view and variant |
|---|---|---|---|---|
| `multi-draw` | `indirect_first_instance` and `multi_draw_indirect` (native) | `base` | 0 | one `multi_draw_indexed_indirect` over every mesh |
| `first-instance` | `indirect_first_instance` without native multi-draw (a browser with the feature) | `base` | 0 | one `draw_indexed_indirect` per batch holding instances |
| `baseline` | no `indirect_first_instance` (WebGPU's defaults) | 0 | `base` | one `set_bind_group` with a dynamic offset and one `draw_indexed_indirect` per batch holding instances |

A batch "holds instances" when the scene has an instance of that mesh and variant; whether the
view's culling kept any of them is known only on the GPU, so such a batch is drawn with whatever
count culling wrote, possibly 0. Batches with no instances in the scene are skipped on the two
per-batch paths, which matters most in the browser, where every call crosses from WebAssembly to
JavaScript.

### 2.3 The base uniform

Group 3, binding 1 of the forward, shadow and id pipelines is `var<uniform> batch: vec4u`, a
16-byte binding with a dynamic offset into one uniform buffer (binding 0 of group 3 is the ocean's,
in its own pipeline layout, so the two never collide). Entry 0 is zero; on the baseline path
entry `1 + view * L + j` holds the base of live batch `j` of view `view`, where `L` is the number of
live batches. Entries are `minUniformBufferOffsetAlignment` apart (256 bytes on WebGPU's defaults),
so the buffer is `(1 + 5L) * 256` bytes, rewritten whenever the counts change. The id pass's
pipeline layout is `[frame, empty, empty, batch]`; it binds the empty groups itself.

### 2.4 What does not need this

Direct draws may use any `first_instance` on WebGPU, so the selection outline
(`overlay.rs`: `draw_indexed(range, base_vertex, slot..slot + 1)`), particles, the sky, the UI and
the ocean are unaffected. The splat renderer's single indirect draw writes `first_instance` 0
already (docs/spec/splats.md).

## 3. Default limits

The renderer's pipelines fit WebGPU's default limits (checked natively by
`POCKET_GPU_MINIMAL=limits`, 4), with little headroom in two places:

- storage buffers per shader stage: the forward pipeline's fragment stage sees 8 of 8 (the frame
  group's four, visible to both stages, and the lighting group's lights, cluster lists and the two
  GI probe buffers). One more storage buffer in either group fails pipeline creation on the
  defaults;
- bind groups: 4 of 4 in the forward, shadow, id and ocean pipelines.

## 4. Emulating a browser natively

`POCKET_GPU_MINIMAL` (natively) and the viewport page's `?gpu_minimal=` (in the browser) take a
comma-separated list (`gpu::Minimal`):

| Token | Effect |
|---|---|
| `features` | no optional features at all |
| `first-instance` | no `indirect-first-instance` only (forces the `baseline` path) |
| `multi-draw` | no native multi-draw (forces the `first-instance` path natively) |
| `timestamps` | no timestamp queries |
| `limits` | WebGPU's default limits (keeping the adapter's resolution limits) |

When `indirect-first-instance` is left out, the instance turns on wgpu-core's indirect-call
validation (`InstanceFlags::VALIDATION_INDIRECT_CALL`) even in release builds, which rewrites an
indirect draw with a nonzero `first_instance` into a no-op as the WebGPU specification requires.
Without it the NVIDIA Vulkan driver drew such draws correctly although the device lacked the
feature, and a native run could not show the browser's failure. The emulation is exact: before the
fix, the mixed scene showed the same 6 of 59 entities natively on Vulkan and in Chrome on D3D12
(`docs/evidence/webgpu/native-prefix`, `browser-prefix`).

## 5. Checks

- `crates/pocket-render/tests/draw_paths.rs` renders the mixed demo scene (`demo::mixed`: the seven
  primitives and a procedural model with one material per variant under a shadow-casting sun) plus
  three skinned characters on the three paths and requires the same entities with the same id-pass
  coverage and nearly the same pixels; it skips without a GPU. Putting the base back into
  `first_instance` on the baseline path makes it fail (0 of 36 entities).
- `cargo run --release -p pocket-app --example draw_paths -- [--minimal ...] mixed cubes
  samples/anim samples/sailing` compares a full-feature device with an emulated one on any scene or
  project and writes images and `draw_paths.json`.
- `python3 tools/web_draw_paths.py <viewport url> --name <n> --out <dir>` runs a viewport page in
  headless Chrome with and without `gpu_minimal=first-instance` and compares coverage and
  screenshots.

## 6. Open

- The id pass's readback in Chrome took 150 to 980 ms on the viewport pages and 50 to 100 ms on the
  game pages under an uncapped frame loop (natively one or two frames), which the editor's picking
  and agents' `render.visible` would feel in the browser.
- The baseline path's CPU cost at thousands of live batches is unmeasured: the largest scene here
  has 31 meshes (samples/anim, each skinned part a mesh of its own).
- A scene whose instance counts change every tick rewrites the template and, on the baseline path,
  `(1 + 5L) * 256` bytes of bases each time.
- WebGPU immediates (an Intent to Ship in Chrome since 2026-05, per
  docs/research/2026-10-04-engine-comparison.md) could replace the base uniform on the baseline
  path; wgpu 30's browser backend panics in `set_immediates`, so that needs a newer wgpu.
