# Gaussian splats

Status: Draft, implemented on branch `feat/splat` (2026-10-04)

Charter: 4.4 (neural rendering: 3D Gaussian Splatting with a preprocess compute shader for
projection, culling and spherical harmonics, a GPU radix sort and instanced quad blending composited
with the mesh depth buffer), 4.1 (one renderer for Metal, Vulkan and browser WebGPU; WebGPU's
defaults as the baseline), 3 item 7 (GPU-driven, no silent capacity limits).

This specification describes how the renderer draws 3D Gaussian Splatting (3DGS) clouds next to
meshes: the files it reads, the GPU data layout, the passes, the sort, the WebGPU constraints the
design keeps, what it costs and what it does not do yet. The code is `crates/pocket-render/src/splat/`
(`mod.rs` the passes, `cloud.rs` the packing, `loader.rs` the files, `sort.rs` the radix sort) and
`crates/pocket-render/shaders/splat_*.wgsl`. The measurements are in
[docs/bench/splats.md](../bench/splats.md).

## 1. Data flow

1. A world entity has a `Splat` component (`pocket-assets/src/visual.rs`: `asset`, uniform `scale`,
   `visible`) and a `Transform`.
2. The render feed (`pocket-runtime/src/present.rs`) sends the full list of `SplatView`s (id, asset,
   pose with the scale, visible) in `RenderFrame.splats` whenever any splat changed.
3. The renderer's `Scene` keeps the list; `Splats::prepare` turns it into the drawn clouds (asset name
   and model matrix) when it changed.
4. Clouds are assets by name in `Renderer::splats`:
   - natively, `Splats::set_root(dir)` starts a worker thread that reads and decodes `.ply` and
     `.splat` files under `dir` when the feed first names them (`pocket play` sets the project
     directory), so a large file never stalls a frame;
   - in the browser, `Splats::take_requests` hands the names to the page with the model paths
     (`Viewport::take_asset_requests`), and the page's fetch comes back through `deliver_asset`, which
     sends `.ply`/`.splat` to `Splats::deliver`;
   - any host (the browser after fetching, an example after generating) calls
     `Splats::insert(name, &SplatCloud)` with a cloud it decoded (`splat::loader::parse`) or built
     (`SplatCloud::from_raw`).
   A cloud is uploaded once and kept for the renderer's lifetime; the same asset drawn by several
   entities is stored once.
5. Each frame with at least one drawn cloud: preprocess, sort, depth copy, draw (section 4).

## 2. Files

### 2.1 3DGS training output (`.ply`)

`binary_little_endian` PLY with a `vertex` element whose properties are found by name (any order,
any scalar type; other elements before it are skipped):

| Property | Meaning | Decoded as |
|---|---|---|
| `x y z` | center | position |
| `nx ny nz` | unused (zeros in training output) | ignored |
| `f_dc_0..2` | SH band 0, per channel | color = 0.5 + 0.2820948 f_dc (display-encoded) |
| `f_rest_0..` | SH bands 1-3, **channel-major** (all red coefficients, then green, then blue); 9, 24 or 45 of them for degree 1, 2, 3 | reordered coefficient-major, RGB interleaved |
| `opacity` | logit | sigmoid |
| `scale_0..2` | log standard deviations | exp |
| `rot_0..3` | quaternion `w x y z`, not necessarily normalized | normalized `x y z w` |

Rejected with a structured message: ASCII or big-endian PLY, list properties, a missing required
property, an `f_rest` count that is not 0, 9, 24 or 45. Not supported yet: compressed PLY
(SuperSplat's chunked quantization), `.spz`, `.ksplat`.

### 2.2 antimatter15 `.splat`

32 bytes per splat, no header: position `f32x3`, linear scale `f32x3`, color `u8x4` (RGB =
(0.5 + 0.2820948 f_dc) x 255, A = sigmoid(opacity) x 255), rotation `u8x4` (`w x y z`, q x 128 + 128).
No spherical harmonics.

### 2.3 Writing

`loader::write_ply` writes the 3DGS layout from `RawSplat`s (log scale, logit opacity, f_dc from
the color, channel-major `f_rest`), so generated clouds can be saved and loaded like captured ones
(`--save` in the example). The round trip is a unit test.

## 3. GPU data layout

All buffers are storage buffers read through plain `u32`/`f32` arrays: no `f16` shader type
(that needs the optional `shader-f16` feature); halves are packed with `pack2x16float` and read with
`unpack2x16float`, which are core WGSL.

| Buffer | Per | Bytes | Contents |
|---|---|---|---|
| splats | stored splat | 32 | position `f32x3`; rotation as smallest-three in one `u32` (2-bit index of the dropped largest component, three 10-bit components over [-1/sqrt 2, 1/sqrt 2], error about 0.001); scale x, y, z and opacity as four `f16`; base color r, g, b as three `f16` (one `f16` spare) |
| SH | stored splat with SH | 4 x ceil(3 x ((d+1)^2 - 1) / 2): 20, 48, 92 for degree 1, 2, 3 | `f16` pairs, coefficient-major, RGB interleaved; each cloud has its own degree and offset |
| clouds | drawn cloud | 112 | model-view matrix, camera position in the cloud's frame, first thread, first splat, count, SH degree, SH offset and words per splat |
| params | frame | 80 (uniform) | viewport, projection terms (P00, P11, P22, P32), near, key range (log2 near, 1/log2 range, largest key), counts, radiance |
| projected | visible splat | 24 | center in NDC `f32x2`; the ellipse's two axes in NDC at one standard deviation as `f16x2` each; linear RGB and opacity as four `f16` |
| keys, values | visible splat, x2 | 4 + 4, twice | sort keys and indices into `projected`, ping-pong |
| histogram | sort tile | 1 KiB per 4096 visible splats | per-tile digit counts, then offsets; digit totals |
| control | frame | 64 (storage + indirect) | `[0]` visible count, `[1]` quad area (16-pixel units), `[4..7]` the sort's indirect dispatch, `[8..13]` the indexed indirect draw |

Per splat that is drawn: 32 stored + 24 projected + 16 keys and values = 72 bytes, plus SH. 1M
garden splats (no SH) use 94 MB of GPU buffers, 3M use 261 MB. Clouds are appended to the splats
and SH buffers (grown by copying on the GPU, at least 1.5x), so no CPU copy is kept.

## 4. Passes

Per frame, recorded by `Splats::prepare` (before the opaque pass) and `Splats::draw` (after it):

1. **CPU, per drawn cloud only.** Transform the cloud's bounds (centers grown by three standard
   deviations of its largest splat) by its pose; skip the cloud if the box is outside one frustum
   plane; collect the nearest and farthest view depth of all drawn boxes (the key range); write the
   cloud table and the params. Nothing per splat touches the CPU.
2. **`splat preprocess`** (compute, 256 threads, one thread per splat of every drawn cloud; a 2D
   dispatch above 65,535 workgroups). A thread finds its cloud by binary search over the clouds'
   first threads, then:
   - view position t = model-view x center; culled if its depth tz = -t.z is not beyond the near
     plane or its opacity is below 1/255;
   - 3D covariance as M M^T with M = A R S (A the model-view's linear part, so the pose's rotation
     and non-uniform scale apply; R from the quaternion; S the scales);
   - 2D covariance G G^T + 0.3 I in pixels, G = J M, J the perspective Jacobian at the center
     (rows (fx/tz, 0, fx x/tz) and (0, fy/tz, fy y/tz) with x, y the center's tangents clamped to 1.3x
     the frustum's, as the reference implementation does); the 0.3 px^2 low-pass matches training;
   - eigenvalues l1 >= l2 >= 0.1 and the major eigenvector give the ellipse's axes; the quad extends
     k = min(3, sqrt(2 ln(255 opacity))) standard deviations, where the Gaussian times the opacity
     falls to 1/255;
   - culled if the center is outside the screen by more than k sqrt(Sigma_xx), k sqrt(Sigma_yy);
   - color = base + SH bands 1..d toward the camera (direction in the cloud's frame, the reference
     implementation's constants and signs), clamped at 0, converted from display encoding to linear
     light (trained colors are display-encoded; the target is linear HDR), times `radiance`;
   - the depth key (section 5.1).
   Visible splats are appended: a workgroup counts its visible splats in workgroup memory and one
   thread reserves the range with one global atomic, so 3M splats cost 12k global atomics.
   A one-thread `finish` dispatch then writes the sort's indirect dispatch (one workgroup per 4096
   keys) and the draw's arguments from the visible count. The CPU never reads the count back on the
   frame's path (a ring readback a few frames late feeds `SplatStats::visible`).
3. **`splat sort`** (compute): section 5.
4. **opaque+sky** (the existing pass) keeps its multisampled depth (`StoreOp::Store` instead of
   `Discard`) when splats draw this frame.
5. **`splat depth`**: a full-screen pass writes each pixel's first depth sample into a
   single-sample `Depth32Float` target (`textureLoad` of `texture_depth_multisampled_2d`, written
   through `frag_depth`).
6. **`splat draw`**: one `draw_indexed_indirect` into the resolved single-sample HDR image, depth
   test `Greater` (reversed-Z) against the copied depth, depth writes off, blending premultiplied
   (`One, OneMinusSrcAlpha`). An instance is a batch of 16,383 quads from a fixed 16-bit index
   buffer (four vertices, two triangles per quad); the vertex shader takes splat `i = instance x
   16383 + vertex / 4` in sorted order, reads its key (the depth: z_ndc = (P32 - P22 tz) / tz) and
   its projected record, and places the corner at center + (+-a1 +- a2) k. The fragment shader
   evaluates alpha = min(opacity 2^(-|uv|^2 / (2 ln 2)), 0.99) with uv in standard deviations and
   writes (rgb alpha, alpha); below 1/255 it writes zero instead of discarding.

Why after the opaque pass and single-sampled: blending the same splats into the 4x multisampled
target inside the opaque pass measured 2.2x slower on the Apple M5 (16.7 ms against 7.7 ms for 1M
splats at 1600x900), because blending cost is per sample. The cost of the single-sample path is
the depth copy (about 0.1-1 ms) and keeping the multisampled depth, and mesh silhouettes in front
of splats are resolved per pixel (the first sample), not per sample. A `discard` below 1/255
measured 1.5x slower than blending a zero (Apple GPUs leave their fast path for shaders that
discard).

### 4.1 Hook points in the renderer

`crates/pocket-render/src/renderer.rs`:

- the field `pub splats: crate::splat::Splats` and `splats: crate::splat::Splats::new(gpu)` in `new`;
- before the opaque pass: `self.splats.prepare(&mut enc, &mut self.profiler, &mut self.scene, &cam, (w, h));`
- the opaque pass's depth `store: if self.splats.active() { StoreOp::Store } else { StoreOp::Discard }`;
- after the opaque pass and the entity-id pass, before bloom:
  `self.splats.draw(&mut enc, &mut self.profiler, &self.targets);`

`post.rs` gives the multisampled depth `TEXTURE_BINDING` usage; `shaders.rs` composes the splat
shaders (`splat_common.wgsl` is prepended to the preprocess and the draw); `pocket-app`'s play window
calls `splats.set_root(project_dir)`.

## 5. The sort

### 5.1 Keys

Ascending keys run back to front. With `key_bits = 32` the key is the bitwise inverse of the depth's
float bits (exact: positive floats order like their bits). With 16 or 24 bits (2 or 3 passes) the
key is the depth's log2 quantized over the frame's key range from step 1:
key = round((1 - (log2 tz - log2 near) / (log2 far - log2 near)) (2^bits - 1)). For a range of
0.1 to 40 m (log2 range 8.6) a 24-bit step is 3.6e-7 of the depth and a 16-bit step 9.1e-5 (0.9 mm
at 10 m). The draw recovers the quad's depth from the key. Default: 24 bits.

### 5.2 Algorithm

A least-significant-digit radix sort of (key, value) `u32` pairs, 8-bit digits, stable, using only
WebGPU core features: workgroups of 256 threads, 10 KiB of workgroup memory, no subgroup
operations, at most 6 storage buffers. Each pass sorts by the digit at `shift` over tiles of 4096
keys (16 rounds of 256), in three dispatches:

1. `histogram` (indirect, one workgroup per tile): workgroup-atomic counts of the tile's 256 digits,
   stored digit-major (`hist[digit x tiles + tile]`).
2. `scan_bins` (256 workgroups, one per digit): each thread sums a contiguous run of tiles, a
   Hillis-Steele workgroup scan, then the run is rewritten as exclusive offsets; the digit's total
   goes after the table.
3. `scatter` (indirect, one workgroup per tile): a workgroup scan of the 256 totals gives each
   digit's start; each round, every thread with a key sets its bit in a 256-bit mask of its digit
   (256 digits x 8 words of workgroup memory, `atomicOr`); its rank among the round's keys of its
   digit is the popcount of the mask's lower bits; it writes key and value to digit start + tile
   offset + earlier rounds' count of the digit + rank; the thread owning each digit then adds the
   round's count and clears the mask. Three barriers per round. This is the warp-level multisplit
   idea done in workgroup memory, so ranks are stable without subgroup ballots.

The key count and the tile count come from the control buffer the preprocess wrote; the pass shift
is a dynamic offset into a uniform buffer of four 256-byte slots. Two bind groups alternate the
buffers; the result is in buffer A after an even number of passes and B after an odd one. A GPU
unit test sorts random keys with duplicates (1 to 100,003 keys, 2 to 4 passes) and compares keys and
values with a stable CPU sort.

Measured at 3M splats (2.46M visible, 24 bits, 3 passes): histogram 0.61 ms, scan 0.17 ms, scatter
2.23 ms in total; the scatter's scattered 4-byte writes dominate. Ways to cut it: a local reorder
in workgroup memory before writing (coalesced runs), a 16-bit key (2 passes, 1.9 ms), and on native
adapters with subgroups a ballot-based multisplit and an Onesweep-style single pass with decoupled
look-back.

## 6. WebGPU constraints

| Constraint (WebGPU default) | Use |
|---|---|
| storage buffers per stage (8) | preprocess 7, sort 6, draw vertex stage 4 |
| workgroup memory (16 KiB) | scatter 10 KiB, histogram 1 KiB, preprocess 8 bytes |
| invocations per workgroup (256) | 256 everywhere |
| workgroups per dimension (65,535) | preprocess spills into y; sort tiles are 4096 keys (268M keys per dispatch) |
| storage binding size (128 MiB) | splats 32 B: 4.19M per cloud set; projected 24 B: 5.59M visible; SH degree 3 at 92 B: 1.46M splats. `Gpu::new` asks for the adapter's limits, which browsers raise on request (2-4 GiB on desktop Chrome) |
| no `shader-f16`, no subgroups, no `INDIRECT_FIRST_INSTANCE` | halves through `pack2x16float`; workgroup-memory multisplit; the draw's batches use `instance_index`, `first_instance` is 0 |
| no multi-draw indirect | one indexed indirect draw |
| uniformity analysis | barriers only in uniform control flow; counts read from read-only storage; `workgroupUniformLoad` for the preprocess's base |
| timestamps optional | the profiler's scopes are absent without them |
| workgroup memory zeroing | off natively (`shaders::compute_options`, it breaks MoltenVK); every shader initializes what it reads |

Verified: Metal and Vulkan (MoltenVK) on the Apple M5 render the same image (mean absolute difference
0.0003 of 255); with `POCKET_GPU_MINIMAL=limits,features` (WebGPU's default limits, no optional
features) the 1M and 3M gardens render; the crate builds for `wasm32-unknown-unknown`. Not yet run
in a browser.

## 7. Test content

No captured scenes are downloaded (the owner has not approved downloads). The example
`crates/pocket-render/examples/splats.rs` generates a garden: rolling terrain covered with grass
discs and a dirt path (36% of the splats), grass blades (20%), seven flower beds of stem, petal and
center splats (20%), three trees with bark discs and leaf crowns (10%), a beach ball (7%) and a
standing rainbow ring (7%), all oriented flat Gaussians whose size follows the sampling density;
plus a 40k-splat orb with degree-3 spherical harmonics (view-dependent color) drawn twice at
different poses, lit primitive meshes in and around them (pedestal, crates, a pillar through the
ring, a sphere) and the procedural sky. `--count N` sets the garden's size, `--ply PATH` draws a
file instead, `--save PATH` writes the garden as a 3DGS `.ply`. Screenshots:
`docs/evidence/render/splat_garden.png` (1M), `splat_garden_3m.png` (3M at WebGPU's default
limits), `splat_closeup.png` (meshes and splats occluding each other), `splat_sh.png` (the orb from
opposite sides), `splat_play.png` (a `Splat` component in the sailing sample loaded from a `.ply`
through the runtime's feed, cut by the ocean's depth).

### 7.1 Fitting splats to the renderer's own images

`tools/neural/splatfit/fit.py` (uv; PyTorch on Apple MPS, CUDA or the CPU; no downloaded data)
closes the loop from meshes to splats and back:

1. `splats --dataset DIR --views 48 --size 160x160` renders a small mesh scene (a ground plane, a
   sphere, a crate, a pillar, a torus, a cone, a shadowing sun, a flat sky, no bloom) from 48
   cameras on a ring at varying heights, with `cameras.txt` (eyes, targets, field of view, the
   exposure multiplier and the sky color).
2. `uv run fit.py DIR` fits Gaussians with a differentiable tile rasterizer written in plain
   PyTorch: the preprocess's projection (EWA, 0.3 px^2 low-pass, 3-sigma extent), 16x16 tiles each
   composited front to back over its nearest K = 1,024 Gaussians, the sky behind, then the renderer's
   display transform (exposure and the AgX of post.wgsl, ported) so the L1 loss compares what the
   engine shows. Colors are linear radiance, written as display-encoded SH band 0. Initialization is
   random (half on the ground, half in the objects' volume: there is no structure-from-motion);
   densification is a relocation of transparent Gaussians next to opaque ones every 300 steps
   (optional L1 penalties on opacity and scale, off by default). Every eighth view is held out.
3. `splats --replay DIR --ply DIR/fitted.ply` draws the result with the engine from all 48 cameras
   and `fit.py DIR --score` reports the PSNR against the references.

Status: works end to end at a small scale. With 30,000 Gaussians, 160x160 images, 42 training views
and 2,500 steps (17 minutes on the M5's GPU through MPS, shared with other work), the engine's
renders of the fitted `.ply` reach a mean PSNR of 34.7 dB on the 6 held-out views and 36.7 dB on
the training views (the fitter's own renders: 31.5 and 33.7 dB). `docs/evidence/render/splat_fit.png`
shows held-out references above the engine's renders. Floaters remain where few views see (below
the ground plane's near edge). What it is not: a 3DGS trainer for real captures. There is no
structure-from-motion initialization, no adaptive density control beyond relocation, no SSIM term,
no spherical harmonics above band 0, and the PyTorch rasterizer is dense per tile (about 0.4 s a
step at 160x160), so megapixel images or millions of Gaussians need a fused kernel (gsplat or a
compute-shader trainer). Each tile composites its 1,024 nearest Gaussians; at the end of the run
39% of tile-Gaussian pairs were beyond that; they lie mostly behind layers that are already
opaque, since the engine, which draws them all, scores higher than the fitter. An earlier run with K = 320 and opacity and scale
penalties of 0.01 scored 21.9 dB in the engine: truncating semi-transparent layers that the engine
then draws shows up as colored noise.

## 8. Known limits

- **Fill-bound.** A quad rasterizer blends every fragment of every visible splat; it cannot stop at
  a saturated pixel. The garden's quads cover 32x (1M) and 64x (3M) the screen, and the draw is
  most of the frame (4-6 ms and 21-30 ms at 1600x900 on the M5). The next step is a compute tile
  rasterizer (per-tile lists sorted by depth, front-to-back with early termination, as the
  reference CUDA rasterizer and Brush do), or level of detail for distant splats.
- **No lighting or shadows.** Splats carry baked radiance (times `radiance`); they neither cast nor
  receive the sun's shadows and are not lit by scene lights.
- **No anti-aliasing compensation.** No Mip-Splatting 3D filter or opacity compensation for the 0.3
  px low-pass, as in the original 3DGS.
- **Depth.** A splat is depth-tested at its center's depth, so a splat straddling a mesh surface is
  either drawn whole or hidden; mesh silhouettes over splats are per pixel (section 4).
- **Ties.** Splats with equal keys keep the preprocess's append order, which varies between frames;
  with 16-bit keys dense clouds have many ties and can flicker slightly where they overlap.
- **Assets.** Clouds are never freed; inserting a name again leaves the old data unused. No
  streaming, no compressed formats, no level of detail; `.ply` files are read whole.
- **Poses** are not interpolated between ticks (the feed's `SplatView` has one pose).
- **Picking.** Splats are not in the entity-id pass, so picking and view coverage do not see them.
- **Other transparency.** Splats draw after the opaque pass, so GPU particles (drawn inside it)
  are covered by splats even where they are in front of them; the two are not sorted together.
- **Browser**: verified in headless Chrome (WebGPU) through the web player (`web/`, the game in a
  Worker): a 300k-splat garden `.ply` fetched by the page, drawn with the anim sample's skinned
  characters, UI and particles; GPU 4.1 ms per frame at 1280x900 window size of which splat draw
  2.5 ms, preprocess 0.16 ms, sort 0.27 ms (`docs/evidence/render/web-player-splats.png`).
