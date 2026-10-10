# Gaussian splats

Status: Draft, implemented on branch `feat/splat` (2026-10-04); the compute tile rasterizer, the
anti-aliased mode, splats in the entity-id pass and stable ties on `explore/splats` (2026-10-09,
Amoris Pioneer); the quads stay the default rasterizer on every adapter (2026-10-10, after the quiet
re-measurement, `explore/quiet2`)

Charter: 4.4 (neural rendering: 3D Gaussian Splatting with a preprocess compute shader for
projection, culling and spherical harmonics, a GPU radix sort and instanced quad blending composited
with the mesh depth buffer; since 2026-10-09 a compute tile rasterizer beside it, a Pioneer
decision), 4.1 (one renderer for Metal, Vulkan and browser WebGPU; WebGPU's defaults as the
baseline), 3 item 7 (GPU-driven, no silent capacity limits).

This specification describes how the renderer draws 3D Gaussian Splatting (3DGS) clouds next to
meshes: the files it reads, the GPU data layout, the passes, the sort, the WebGPU constraints the
design keeps, what it costs and what it does not do yet. The code is
`crates/pocket-render/src/splat/` (`mod.rs` the passes, `cloud.rs` the packing, `loader.rs` the
files, `sort.rs` the radix sort, `tile.rs` the tile rasterizer) and
`crates/pocket-render/shaders/splat_*.wgsl`. The measurements are in
[docs/bench/splats.md](../bench/splats.md).

## 1. Data flow

1. A world entity has a `Splat` component (`pocket-assets/src/visual.rs`: `asset`, uniform `scale`,
   `visible`) and a `Transform`.
2. The render feed (`pocket-runtime/src/present.rs`) sends the full list of `SplatView`s (id, asset,
   pose with the scale, visible) in `RenderFrame.splats` whenever any splat changed.
3. The renderer's `Scene` keeps the list; `Splats::prepare` turns it into the drawn clouds (asset
   name and model matrix) when it changed.
4. Clouds are assets by name in `Renderer::splats`:
   - natively, `Splats::set_root(dir)` starts a worker thread that reads and decodes `.ply` and
     `.splat` files under `dir` when the feed first names them (`pocket play` sets the project
     directory), so a large file never stalls a frame;
   - in the browser, `Splats::take_requests` hands the names to the page with the model paths
     (`Viewport::take_asset_requests`), and the page's fetch comes back through `deliver_asset`,
     which sends `.ply`/`.splat` to `Splats::deliver`;
   - any host (the browser after fetching, an example after generating) calls
     `Splats::insert(name, &SplatCloud)` with a cloud it decoded (`splat::loader::parse`) or built
     (`SplatCloud::from_raw`).
A cloud is uploaded once and kept for the renderer's lifetime; the same asset drawn by several
entities is stored once.
5. Each frame with at least one drawn cloud: preprocess, compaction, sort, then either the quads
   (depth copy, draw) or the tile rasterizer (binning and the pair sort before the opaque pass,
   raster and composite after it) (section 4). On frames that answer a pick or a coverage request
   the splats also draw into the entity-id pass (section 4.4).

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

32 bytes per splat, no header: position `f32x3`, linear scale `f32x3`, color `u8x4` (RGB = (0.5 +
0.2820948 f_dc) x 255, A = sigmoid(opacity) x 255), rotation `u8x4` (`w x y z`, q x 128 + 128). No
spherical harmonics.

### 2.3 Writing

`loader::write_ply` writes the 3DGS layout from `RawSplat`s (log scale, logit opacity, f_dc from the
color, channel-major `f_rest`), so generated clouds can be saved and loaded like captured ones
(`--save` in the example). The round trip is a unit test.

## 3. GPU data layout

All buffers are storage buffers read through plain `u32`/`f32` arrays: no `f16` shader type (that
needs the optional `shader-f16` feature); halves are packed with `pack2x16float` and read with
`unpack2x16float`, which are core WGSL.

| Buffer | Per | Bytes | Contents |
|---|---|---|---|
| splats | stored splat | 32 | position `f32x3`; rotation as smallest-three in one `u32` (2-bit index of the dropped largest component, three 10-bit components over [-1/sqrt 2, 1/sqrt 2], error about 0.001); scale x, y, z and opacity as four `f16`; base color r, g, b as three `f16` (one `f16` spare) |
| SH | stored splat with SH | 4 x ceil(3 x ((d+1)^2 - 1) / 2): 20, 48, 92 for degree 1, 2, 3 | `f16` pairs, coefficient-major, RGB interleaved; each cloud has its own degree and offset |
| clouds | drawn cloud | 112 | model-view matrix, camera position in the cloud's frame, first thread, first splat, count, SH degree, SH offset and words per splat |
| params | frame | 96 (uniform) | viewport, projection terms (P00, P11, P22, P32), near, key range (log2 near, 1/log2 range, largest key), counts and flags (`SPLAT_ANTIALIAS`), radiance, the tile rasterizer's tiles across and down and pair capacity |
| projected | submitted splat (thread) | 24 | written for visible splats at their thread index: center in NDC `f32x2`; the ellipse's two axes in NDC at one standard deviation as `f16x2` each; linear RGB and opacity as four `f16` |
| keys, values | submitted splat, x2 | 4 + 4, twice | sort keys and thread indices (into `projected`), ping-pong; keys B first stages the preprocess's keys by thread index |
| blocks | 256 submitted splats | 4 | a preprocess workgroup's visible count, then its first slot |
| histogram | sort tile | 1 KiB per 4096 visible splats | per-tile digit counts, then offsets; digit totals |
| control | frame | 64 (storage + indirect) | `[0]` visible count, `[1]` quad area (16-pixel units), `[4..7]` the sort's indirect dispatch, `[8..13]` the indexed indirect draw |

Per splat that is drawn: 32 stored + 24 projected + 16 keys and values = 72 bytes, plus SH. 1M
garden splats (no SH) use 94 MB of GPU buffers, 3M use 261 MB. Clouds are appended to the splats and
SH buffers (grown by copying on the GPU, at least 1.5x), so no CPU copy is kept.

The tile rasterizer (section 4.2) adds, allocated only once it is selected:

| Buffer | Per | Bytes | Contents |
|---|---|---|---|
| tile splats | visible splat | 32 | in front-to-back order: center in pixels `f32x2`, both axes divided by their squared length as `f16x2` each (`dot(d, a / \|a\|^2)` is the offset in standard deviations), linear RGB and opacity as four `f16`, the reversed-Z depth the quads test, the extent k |
| rects | visible splat | 16 | the tile rectangle of the quad's bounding box and how many of its tiles the quad overlaps |
| tile blocks | 256 visible splats | 4 | pair counts, then first pair |
| pair keys, values | pair, x2 | 4 + 4, twice | tile ids and front-to-back splat indices, ping-pong for the sort |
| pair histogram | 4096 pairs | 1 KiB | the radix sort's |
| ranges | screen tile | 8 | first and end pair |
| tile control | frame | 64 (storage + indirect) | `[0]` pairs to sort, `[1]` pairs wanted, `[4..6]` the pair sort's dispatch, `[8..10]` the dispatch over the visible splats |
| tile image | pixel | 8 (`rgba16float` storage texture) | premultiplied color and transmittance |

The pair buffers start at three pairs per submitted splat and grow (by at least 1.25x) from a
readback of the wanted count, up to the device's storage binding size (section 4.2). With them the
1M garden uses 198 MB and the 3M garden 564 MB of splat buffers.

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
   - 2D covariance G G^T + 0.3 I in pixels, G = J M, J the perspective Jacobian at the center (rows
     (fx/tz, 0, fx x/tz) and (0, fy/tz, fy y/tz) with x, y the center's tangents clamped to 1.3x the
     frustum's, as the reference implementation does); the 0.3 px^2 low-pass matches training;
   - eigenvalues l1 >= l2 >= 0.1 and the major eigenvector give the ellipse's axes; the quad extends
     k = min(3, sqrt(2 ln(255 opacity))) standard deviations, where the Gaussian times the opacity
     falls to 1/255;
   - culled if the center is outside the screen by more than k sqrt(Sigma_xx), k sqrt(Sigma_yy);
   - color = base + SH bands 1..d toward the camera (direction in the cloud's frame, the reference
     implementation's constants and signs), clamped at 0, converted from display encoding to linear
     light (trained colors are display-encoded; the target is linear HDR), times `radiance`;
   - in the anti-aliased mode the opacity is compensated for the low-pass first (section 4.3);
   - the depth key (section 5.1).
A visible splat writes its projected record at its thread index and its key into a staging array
(keys B); an invisible one writes `NOT_VISIBLE` (all ones: no depth key is). Each workgroup stores
its visible count.
3. **`splat compact`** (compute): `scan` (one workgroup) turns the workgroups' counts into first
   slots and writes the visible count; `compact` (the preprocess's grid) writes each visible splat's
   key and thread index at its workgroup's first slot plus its rank among the workgroup's visible
   splats, taken from a 256-bit mask in workgroup memory (no atomic order involved). The sort's
   input is therefore in thread-index order and equal keys keep it (section 5.1). A one-thread
   `finish` dispatch then writes the sort's indirect dispatch (one workgroup per 4096 keys) and the
   draw's arguments from the visible count. The CPU never reads the count back on the frame's path
   (a ring readback a few frames late feeds `SplatStats::visible`). This replaced an append through
   one global atomic per workgroup, whose order changed between frames; it costs 0.04 ms at 1M and
   0.13 ms at 3M on the RTX 5060.
4. **`splat sort`** (compute): section 5.
5. **opaque+sky** (the existing pass) keeps its multisampled depth (`StoreOp::Store` instead of
   `Discard`) when splats draw this frame.

With the quads ([`SplatRaster::Quads`], the default):

6. **`splat depth`**: a full-screen pass writes each pixel's first depth sample into a single-sample
   `Depth32Float` target (`textureLoad` of `texture_depth_multisampled_2d`, written through
   `frag_depth`). With TAA the opaque pass's depth is jittered and the splats are not: the renderer
   draws an unjittered depth into that target instead and this pass is skipped
   ([TAA and GTAO](taa-gtao.md) 1.1).
7. **`splat draw`**: one `draw_indexed_indirect` into the resolved single-sample HDR image, depth
   test `Greater` (reversed-Z) against the copied depth, depth writes off, blending premultiplied
   (`One, OneMinusSrcAlpha`). An instance is a batch of 16,383 quads from a fixed 16-bit index
   buffer (four vertices, two triangles per quad); the vertex shader takes splat
   `i = instance x 16383 + vertex / 4` in sorted order, reads its key (the depth: z_ndc = (P32 - P22
   tz) / tz) and its projected record, and places the corner at center + (+-a1 +- a2) k. The
   fragment shader evaluates alpha = min(opacity 2^(-|uv|^2 / (2 ln 2)), 0.99) with uv in standard
   deviations and writes (rgb alpha, alpha); below 1/255 it writes zero instead of discarding.

Why after the opaque pass and single-sampled: blending the same splats into the 4x multisampled
target inside the opaque pass measured 2.2x slower on the Apple M5 (16.7 ms against 7.7 ms for 1M
splats at 1600x900), because blending cost is per sample. The cost of the single-sample path is the
depth copy (about 0.1-1 ms) and keeping the multisampled depth, and mesh silhouettes in front of
splats are resolved per pixel (the first sample), not per sample. A `discard` below 1/255 measured
1.5x slower than blending a zero (Apple GPUs leave their fast path for shaders that discard).

With the tile rasterizer ([`SplatRaster::Tiles`]): section 4.2.

### 4.1 Hook points in the renderer

`crates/pocket-render/src/renderer.rs`:

- the field `pub splats: crate::splat::Splats` and `splats: crate::splat::Splats::new(gpu)` in
  `new`;
- before the opaque pass:
  `self.splats.prepare(&mut enc, &mut self.profiler, &mut self.scene, &cam, (w, h));`
- the opaque pass's depth
  `store: if self.splats.active() { StoreOp::Store } else { StoreOp::Discard }`;
- in the entity-id pass's draw closure, after the meshes: `splats.draw_ids(pass);` and after the
  pass `self.pick_splats = self.splats.drawn_entities().to_vec();` (section 4.4);
- after the opaque pass and the entity-id pass, before bloom:
  `self.splats.draw(&mut enc, &mut self.profiler, &self.targets);` (quads or tiles).

`post.rs` gives the multisampled depth `TEXTURE_BINDING` usage; `shaders.rs` composes the splat
shaders (`splat_common.wgsl` is prepended to the preprocess, the draw and the tile rasterizer);
`pocket-app`'s play window calls `splats.set_root(project_dir)`.

### 4.2 The compute tile rasterizer

`Splats::raster = SplatRaster::Tiles` (or `POCKET_SPLAT_RASTER=tile`; the example's `--raster tile`)
replaces steps 6 and 7 with the design of the reference CUDA rasterizer, Brush and gsplat, kept to
WebGPU core (`tile.rs`, `splat_tile.wgsl`, `splat_composite.wgsl`). The preprocess, the compaction
and the depth sort are shared; after the sort, still before the opaque pass:

1. **`splat tile count`**: `tile_setup` (one thread) writes the dispatch over the visible splats (2D
   above 65,535 workgroups); `tile_count` runs one thread per visible splat in front-to-back order p
   (sorted index n - 1 - p). It writes the splat's rasterizer record (section 3: center in pixels,
   axes divided by their squared length, color and opacity, the depth z the quads would test,
   recovered from the key, and the extent k) and the rectangle of 16x16-pixel tiles its quad's
   bounding box covers, clamped to the screen. Each tile of the rectangle is then tested exactly
   against the oriented quad (separating axes: x, y and the quad's two axes, in standard deviations
   along each, where a tile's pixel centers project to 7.5 (|ia.x| + |ia.y|)), with the `f16` axes
   the later kernels read; the count of overlapped tiles goes into the rectangle and the workgroup's
   sum into its block. Exact testing cut the pairs from 2.86 to 2.30 per visible splat on the 1M
   garden (the thin grass blades' bounding boxes are mostly empty).
2. **`splat tile scan`** (one workgroup): the blocks become offsets; the pair count, clamped to the
   pair capacity, and the wanted count go to the tile control with the pair sort's dispatch.
3. **`splat tile emit`**: each splat, at its block's offset plus its workgroup scan, writes (tile,
   p) for each overlapped tile, in row order. It writes exactly the stored count: should its own
   test disagree (floating-point contraction can differ between kernels), surplus tiles are dropped
   and missing ones padded with `NO_TILE` (all ones), which sorts after every tile and which the
   range pass ignores. Pairs at or past the capacity are not written: since p runs front to back, an
   overflow drops the farthest splats' tiles first.
4. **`splat tile sort`**: the portable radix sort (section 5) sorts the pairs by tile id, with as
   many 8-bit passes as the tile count needs to keep `NO_TILE`'s sorted bits above every tile (two
   up to 65,535 tiles: 1600x900 has 5,700, 4K 32,400). The sort is stable and the pairs were emitted
   front to back, so every tile's run stays in depth order: a (tile, depth) order without a 64-bit
   key. `tile_ranges` (one workgroup per 4096 pairs) records each tile's first and end pair in a
   range table cleared every frame.

After the opaque pass:

5. **`splat raster`**: one workgroup of 256 invocations per tile. Each pixel reads the scene's depth
   (sample 0 of the multisampled depth, as the quads' depth copy does; with TAA the unjittered
   single-sample depth, [TAA and GTAO](taa-gtao.md) 1.1). The tile's splats are
   processed in batches of 256: each invocation loads one record into workgroup memory (center, z,
   k; axes; color: 12 KiB) and tests the quad against the tile's eight 8x4-pixel sub-tiles, setting
   its bit in each overlapped sub-tile's 256-bit list (`atomicOr` in workgroup memory). A pixel then
   walks its sub-tile's list in order (`firstTrailingBit`), so it visits only splats whose quad can
   reach it; invocations 32s to 32s + 31 are sub-tile s, so on 32-wide hardware a SIMD group walks
   one list in step and its workgroup reads are broadcasts. Per splat: stop at the first one behind
   the scene's surface (z not greater than the depth: every later splat is farther), skip pixels
   outside the quad (|u| or |v| above k, as the quad draws), alpha = min(opacity 2^(-|uv|^2 / (2 ln
   2)), 0.99), skip below 1/255, accumulate color times alpha times transmittance, and stop once the
   transmittance is below 1/255. The workgroup stops when all its pixels have (a count in workgroup
   memory, read uniformly before each batch). It writes (premultiplied color, transmittance) to an
   `rgba16float` storage texture. The walk ends through its loop conditions, without `break` or
   `continue`: with them NVIDIA's Direct3D 12 driver ran it 8x slower and AMD's 1.5x (same pixels;
   bench, quiet re-measurement); AMD's Vulkan driver runs this form 7% slower.
6. **`splat composite`**: a full-screen pass blends the image over the resolved HDR image with
   `One, SrcAlpha`: splats + transmittance x scene (transmittance is stored rather than coverage, so
   `f16` keeps small values exact).

The two rasterizers draw the same Gaussians with the same cut, depth test and alpha, and their
images agree to a PSNR of 57-58.6 dB (mean difference 0.09-0.13 of 255, at most 8-9; the garden at
1M and 3M, the SH orb close-up, the anti-aliased mode, 16-bit keys, WebGPU's default limits:
`out/bench-runs/splats/tile-vs-quad.png`). The remaining difference has two sources: the quads blend
into an `f16` render target and round at every one of up to dozens of layers, the tiles accumulate
in `f32` and round once; and the tiles stop below 1/255 transmittance, dropping what lies behind (at
most 1/255 of it). The residue concentrates where many semi-transparent layers overlap (the tree
crowns).

Capacity: the pair buffers start at three pairs per submitted splat (at least 1M) and grow to 1.25x
the wanted count when a readback a few frames late shows more, up to the storage binding size (a
quarter of it in pairs). Until the readback arrives, a sudden jump in pairs drops the farthest
splats' tiles for a few frames; beyond the device's limit they stay dropped. The readback carries
the tile control's pairs sorted and wanted, so `SplatStats::tile_dropped` is the read frame's own
loss (wanted minus sorted), whether transient or at the limit; `tile_drop_frames` counts the read
frames that lost anything, and the renderer logs once per episode at the limit (transient drops at
debug level). The per-splat buffers (the 32-byte rasterizer records and the 16-byte rectangles)
follow the splat capacity but stop at the binding size too: `tile_setup` bins at most
`params.tiles.w` splats, so beyond it the farthest visible splats are left out of the tiles and
counted in `tile_splats_dropped` (only at the limit: below it the buffers hold every submitted
splat). `limit_tile_pairs` and `limit_tile_splats` lower the limits to test these paths; GPU tests
cover the transient drop (40k large splats against the 1M-pair start), both limits, and growth past
WebGPU's default binding size (`growth_respects_binding_limit`, ignored by default: run it with
`POCKET_GPU_MINIMAL=limits`).

Work counter: with `Splats::count_tests` (params flag `SPLAT_COUNT_TESTS`) the raster adds the
(pixel, splat) pairs its pixels tested to a 64-bit counter (a workgroup sum, then one global atomic
per workgroup), read back as `SplatStats::tile_visits`. It is off by default because the workgroup
sum costs about 6% of the raster; with the flag off the raster runs as fast as without the code.
On the 1M garden at 1600x900 a tile pair costs 34 tests of a possible 256 (116 without the sub-tile
lists); `subtile_culling_limits_tests` asserts the lists cut the tests for small splats below 0.4
of 256 per pair (0.18 measured, 1.0 without them). The test exists because a leftover ablation
(`if (true)` in place of the sub-tile test) shipped in the first commit of the rasterizer and
went unnoticed: it changes no pixel, only the cost (26-48% of the raster on the RTX 5060).

Cost and comparison with the quads: [docs/bench/splats.md](../bench/splats.md). On a quiet machine
(2026-10-10), on the RTX 5060 and the Radeon 780M with Direct3D 12 and Vulkan, the tiles take 1.09
to 1.47 times the quads' GPU time at the garden's default view (1M, 1600x900) and 1.24 to 2.06
times where pixels do not saturate (anti-aliased, the distant view, both; section 8); they win only
with 3M splats or at 2560x1440 and in close-ups (0.79 to 0.97, mostly on the discrete GPU with
Vulkan). On the integrated GPU the binning and the second sort cost about as much as the quads'
whole draw. **The quads are the default on every adapter** (`SplatRaster::DEFAULT`, charter 4.4,
Pioneer 2026-10-10); the tiles stay available (`POCKET_SPLAT_RASTER=tile`, `Splats::raster`).
Apple GPUs, where the quads' blending costs most, are unmeasured: should the M5 measure the tiles
clearly faster, Apple would get its own default.

### 4.3 Anti-aliased mode

`Splats::antialias` (`POCKET_SPLAT_AA=1`; the example's `--antialias`) is gsplat's `antialiased`
rasterization mode, the 2D filter of Mip-Splatting: when the 0.3 px^2 low-pass is added, the opacity
is multiplied by sqrt(det Sigma / det (Sigma + 0.3 I)), so a splat keeps its integral. Without it a
splat smaller than a pixel grows to the low-pass's size at full opacity: thin and distant content
thickens and brightens. The flag rides in the params' `counts.w`; the preprocess culls a splat whose
compensated opacity is below 1/255, and both rasterizers (and the id pass) read the compensated
opacity. Off by default: clouds trained by the original 3DGS learned the plain low-pass and look
thinner with it; clouds trained with compensation (gsplat's antialiased mode, Mip-Splatting) and
generated clouds such as the garden want it on.

Against the same view rendered at 4x the resolution (6400x3600, box-downsampled in linear light),
the garden's region scores 25.0 dB without and 32.8 dB with compensation seen from 2.5x the default
distance, and 27.3 against 33.1 dB from the default distance (the grass blades are below a pixel
there too); the two references, with and without compensation, agree with each other at 37-41 dB, so
the choice of reference does not decide it (`out/bench-runs/splats/aa-crops.png`, `aa-distance.png`,
`summary-2026-10-09.json`).

### 4.4 Entity ids

The entity-id pass (picking.rs, drawn on demand for a pick or a coverage request) draws the frame's
sorted splat quads after the meshes, with their own pipeline (`vs_splat_id`, `fs_splat_id` in
splat_draw.wgsl, sharing the quads' corner code), into its `R32Uint` target and depth: depth test
`Greater` and depth writes on, at the splat center's depth, and a fragment is kept where the
Gaussian reaches half opacity (alpha >= 0.5, `discard` below). A pixel therefore belongs to the
nearest splat that is at least half opaque there, or to a mesh in front of it. The id is
`SPLAT_PICK` (bit 31; mesh ids are slot + 1) or-ed with the splat's drawn cloud, found by binary
search over the clouds table from the sort's value (the thread index, section 4 step 3); the
renderer keeps the drawn clouds' entities from the frame the pass was drawn in (`pick_splats`) and
resolves picks and coverage with them. Independent of the rasterizer. The pass costs about 1.9 ms
with the 1M garden on the RTX 5060 (it draws every quad again, with `discard`), only on frames that
answer a request. On the garden's default view the coverage reports the garden (63% of the view),
both orbs and the meshes (`examples/splats --capture OUT --visible`).

## 5. The sort

### 5.1 Keys

Ascending keys run back to front. With `key_bits = 32` the key is the bitwise inverse of the depth's
float bits (exact: positive floats order like their bits). With 16 or 24 bits (2 or 3 passes) the
key is the depth's log2 quantized over the frame's key range from step 1: key = round((1 - (log2 tz -
log2 near) / (log2 far - log2 near)) (2^bits - 1)). For a range of 0.1 to 40 m (log2 range 8.6) a
24-bit step is 3.6e-7 of the depth and a 16-bit step 9.1e-5 (0.9 mm at 10 m). The draw recovers the
quad's depth from the key. Default: 24 bits.

Ties keep the order of the sort's input, which the compaction (section 4 step 3) makes the thread
index order: the clouds' order and each cloud's splat order, the same every frame. A still view
draws the same image every frame even where every key ties (a GPU test draws a wall of 30,000 splats
at one depth: 39,626 channel values changed over four redraws with the earlier atomic append, none
now, with either rasterizer). Captures are now reproducible bit for bit: the 1M garden rendered
identically in separate runs and with the preprocess forced into a 2D dispatch.

### 5.2 Algorithm

A least-significant-digit radix sort of (key, value) `u32` pairs, 8-bit digits, stable, using only
WebGPU core features: workgroups of 256 threads, 10 KiB of workgroup memory, no subgroup operations,
at most 6 storage buffers. Each pass sorts by the digit at `shift` over tiles of 4096 keys (16
rounds of 256), in three dispatches:

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
buffers; the result is in buffer A after an even number of passes and B after an odd one. A GPU unit
test sorts random keys with duplicates (1 to 100,003 keys, 2 to 4 passes) and compares keys and
values with a stable CPU sort. The tile rasterizer sorts its (tile, splat) pairs with the same
pipelines and its own control buffer and bindings (section 4.2).

Measured at 3M splats (2.46M visible, 24 bits, 3 passes): histogram 0.61 ms, scan 0.17 ms, scatter
2.23 ms in total; the scatter's scattered 4-byte writes dominate. Ways to cut it: a local reorder in
workgroup memory before writing (coalesced runs), a 16-bit key (2 passes, 1.9 ms), and on native
adapters with subgroups a ballot-based multisplit and an Onesweep-style single pass with decoupled
look-back.

## 6. WebGPU constraints

| Constraint (WebGPU default) | Use |
|---|---|
| storage buffers per stage (8) | preprocess 7, compact 4, sort 6, draw vertex stage 4, id pass vertex stage 5, tile kernels at most 7 (each kernel has its own layout: the preprocess's kernels use 9 buffers together) |
| workgroup memory (16 KiB) | scatter 10 KiB, histogram 1 KiB, compact 1 KiB, tile raster 12.3 KiB, tile count and emit 1 KiB |
| invocations per workgroup (256) | 256 everywhere |
| workgroups per dimension (65,535) | preprocess spills into y; sort tiles are 4096 keys (268M keys per dispatch) |
| storage binding size (128 MiB) | splats 32 B: 4.19M per cloud set; projected 24 B: 5.59M submitted (the per-splat buffers grow by half again but stop there; clouds past it are skipped, counted in `SplatStats::skipped` and logged once); SH degree 3 at 92 B: 1.46M splats; tile records 32 B: 4.19M binned splats (rectangles 16 B: 8.39M), beyond which the farthest visible splats are left out of the tiles and reported; tile pairs 4 B: 33.5M pairs, beyond which they are dropped and reported (section 4.2). `Gpu::new` asks for the adapter's limits, which browsers raise on request (2-4 GiB on desktop Chrome) |
| no `shader-f16`, no subgroups, no `INDIRECT_FIRST_INSTANCE` | halves through `pack2x16float`; workgroup-memory multisplit; the draw's batches use `instance_index`, `first_instance` is 0 |
| no multi-draw indirect | one indexed indirect draw |
| uniformity analysis | barriers only in uniform control flow; counts read from read-only storage; `workgroupUniformLoad` for the tile raster's range and stop flag |
| storage textures | the tile image: `rgba16float`, write-only (core) |
| no 64-bit atomics | the (tile, depth) order from a depth sort followed by a stable tile sort |
| timestamps optional | the profiler's scopes are absent without them |
| workgroup memory zeroing | off natively (`shaders::compute_options`, it breaks MoltenVK); every shader initializes what it reads |

Verified: Metal and Vulkan (MoltenVK) on the Apple M5 render the same image (mean absolute
difference 0.0003 of 255); with `POCKET_GPU_MINIMAL=limits,features` (WebGPU's default limits, no
optional features) the 1M and 3M gardens render; the crate builds for `wasm32-unknown-unknown`, and
the web player draws a 300k-splat cloud in Chrome's WebGPU (section 8). 2026-10-09, Windows with the
RTX 5060 (Vulkan): both rasterizers at the default and at WebGPU's default limits; Chrome 155's
WebGPU (Tint, D3D12) compiles every splat module and creates all 17 splat pipelines at its default
limits (`tools/splats/wgsl_check`, `out/bench-runs/splats/chrome-wgsl-check.json`), and the renderer
built for `wasm32` draws 200,000 splats with both rasterizers in it, whose images agree to 55.4 dB
(at most 3 of 255; `tools/splats/web_harness`, `browser-tiles.png`).

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
`out/bench-runs/render/splat_garden.png` (1M), `splat_garden_3m.png` (3M at WebGPU's default
limits), `splat_closeup.png` (meshes and splats occluding each other), `splat_sh.png` (the orb from
opposite sides), `splat_play.png` (a `Splat` component in the sailing sample loaded from a `.ply`
through the runtime's feed, cut by the ocean's depth). `--raster quad|tile` picks the rasterizer,
`--antialias` the compensated low-pass, `--compare PREFIX` captures one view with both rasterizers
and reports their difference, `--visible` (with `--capture`) prints the entity-id pass's coverage
and its cost; `--distance F` and `--look X,Y,Z` apply to the benchmarks too.

### 7.1 Fitting splats to the renderer's own images

`tools/neural/splatfit/fit.py` (uv; PyTorch on Apple MPS, CUDA or the CPU; no downloaded data)
closes the loop from meshes to splats and back:

1. `splats --dataset DIR --views 48 --size 160x160` renders a small mesh scene (a ground plane, a
   sphere, a crate, a pillar, a torus, a cone, a shadowing sun, a flat sky, no bloom) from 48
   cameras on a ring at varying heights, with `cameras.txt` (eyes, targets, field of view, the
   exposure multiplier and the sky color).
2. `uv run fit.py DIR` fits Gaussians with a differentiable tile rasterizer written in plain
   PyTorch: the preprocess's projection (EWA, 0.3 px^2 low-pass, 3-sigma extent), 16x16 tiles each
   composited front to back over its nearest K = 1,024 Gaussians, the sky behind, then the
   renderer's display transform (exposure and the AgX of post.wgsl, ported) so the L1 loss compares
   what the engine shows. Colors are linear radiance, written as display-encoded SH band 0.
   Initialization is random (half on the ground, half in the objects' volume: there is no
   structure-from-motion); densification is a relocation of transparent Gaussians next to opaque
   ones every 300 steps (optional L1 penalties on opacity and scale, off by default). Every eighth
   view is held out.
3. `splats --replay DIR --ply DIR/fitted.ply` draws the result with the engine from all 48 cameras
   and `fit.py DIR --score` reports the PSNR against the references.

Status: works end to end at a small scale. With 30,000 Gaussians, 160x160 images, 42 training views
and 2,500 steps (17 minutes on the M5's GPU through MPS, shared with other work), the engine's
renders of the fitted `.ply` reach a mean PSNR of 34.7 dB on the 6 held-out views and 36.7 dB on the
training views (the fitter's own renders: 31.5 and 33.7 dB). `out/bench-runs/render/splat_fit.png`
shows held-out references above the engine's renders. Floaters remain where few views see (below the
ground plane's near edge). What it is not: a 3DGS trainer for real captures. There is no
structure-from-motion initialization, no adaptive density control beyond relocation, no SSIM term,
no spherical harmonics above band 0, and the PyTorch rasterizer is dense per tile (about 0.4 s a
step at 160x160), so megapixel images or millions of Gaussians need a fused kernel (gsplat or a
compute-shader trainer). Each tile composites its 1,024 nearest Gaussians; at the end of the run 39%
of tile-Gaussian pairs were beyond that; they lie mostly behind layers that are already opaque,
since the engine, which draws them all, scores higher than the fitter. An earlier run with K = 320
and opacity and scale penalties of 0.01 scored 21.9 dB in the engine: truncating semi-transparent
layers that the engine then draws shows up as colored noise.

## 8. Known limits

- **Fill-bound quads.** A quad rasterizer blends every fragment of every visible splat; it cannot
  stop at a saturated pixel. The garden's quads cover 32x (1M) and 64x (3M) the screen, and the draw
  is most of the frame (4-6 ms and 21-30 ms at 1600x900 on the M5; 1.9 and 5.0 ms on the RTX 5060,
  whose fixed-function blending is far cheaper). The compute tile rasterizer (section 4.2) stops
  early but pays for binning and a second sort; level of detail for distant splats would cut both.
- **The tile rasterizer's costs.** Binning (0.4 ms at 1M, 1.2 ms at 3M) and the pair sort (0.45
  and 1.1 ms) come on top of the shared preprocess and depth sort. The raster itself is cheaper than
  the quads' depth copy and draw in every measured view but the distant ones (1M at 1600x900: 1.48
  against 2.09 ms; anti-aliased: 1.74 against 1.89 ms), so where the tiles lose on near views (1M at
  1600x900: 1.10x; anti-aliased: 1.28x) the binning and the second sort decide it. Where pixels do
  not saturate (the garden at 2.5x the distance: sky and thin layers of sub-pixel splats) early
  termination rarely fires and the raster about ties the draw (1.28 against 1.23 ms; the tiles cost
  1.48x the quads); with anti-aliasing there, whose compensated sub-pixel splats are nearly
  transparent, a faint splat still costs the 32 pixels of each 8x4 sub-tile it touches where a quad
  costs its own few, and the raster is 1.7x the draw (tiles 1.89x the quads). `tile_count`'s random
  read of the projected records is its main cost; writing the rasterizer's record in the
  preprocess (the projected record growing to 32 bytes with the depth) would save a 32-byte copy per
  splat. Not tried: smaller tiles, several pixels per invocation, a subgroup path.
- **No lighting or shadows.** Splats carry baked radiance (times `radiance`); they neither cast nor
  receive the sun's shadows and are not lit by scene lights.
- **Anti-aliasing.** The 2D filter with opacity compensation is an option (section 4.3); the
  Mip-Splatting 3D filter (a per-Gaussian smoothing from the training cameras' sampling rates) is
  not implemented, since it belongs to training.
- **Depth.** A splat is depth-tested at its center's depth, so a splat straddling a mesh surface is
  either drawn whole or hidden; mesh silhouettes over splats are per pixel (section 4).
- **Ties** sort by thread index, the same every frame (section 5.1), but that order is arbitrary:
  two splats at one quantized depth blend in cloud order, not by their true depth (24-bit keys make
  this rare).
- **Assets.** Clouds are never freed; inserting a name again leaves the old data unused. No
  streaming, no compressed formats, no level of detail; `.ply` files are read whole.
- **Poses** are not interpolated between ticks (the feed's `SplatView` has one pose).
- **Picking** sees a splat where it alone reaches half opacity (section 4.4). A cloud made only of
  faint splats that saturate together (fog, fuzz) is not picked even where it looks opaque;
  accumulating opacity front to back until it crosses one half, which the tile raster could do for
  free, would fix it. The id pass redraws every quad (1.9 ms at 1M on the RTX 5060).
- **Other transparency.** Splats draw after the opaque pass, so GPU particles (drawn inside it) are
  covered by splats even where they are in front of them; the two are not sorted together.
- **Browser**: verified in headless Chrome (WebGPU) through the web player (`web/`, the game in a
  Worker): a 300k-splat garden `.ply` fetched by the page, drawn with the anim sample's skinned
  characters, UI and particles; GPU 4.1 ms per frame at 1280x900 window size of which splat draw 2.5
  ms, preprocess 0.16 ms, sort 0.27 ms (`out/bench-runs/render/web-player-splats.png`).
