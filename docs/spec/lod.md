# Levels of detail

Status: Draft, implemented on branch `explore/lod` (2026-10-09)

Charter: 4.4 (the Pioneer note of 2026-10-09 on mesh LOD: where levels are made, how they are
picked and drawn, the switch), 4.1 (meshopt for LOD, vertex cache and meshlets), 3 item 7
(GPU-driven).

This specification says how a mesh's coarser levels are made, where they live on the GPU, how the
culling pass picks one per instance and view, how they are drawn on the three draw paths with
occlusion culling, the id pass and the shadow cascades, what that costs, and how it is checked. The
code is `crates/pocket-assets/src/lod.rs` (making levels), `crates/pocket-render/src/meshes.rs`
(the rows), `crates/pocket-render/shaders/cull.wgsl` (picking), `crates/pocket-render/src/lod.rs`
(the switch, the bounds, the read-back counts) and `renderer.rs` (`sync`, `render`). Measurements:
[docs/bench/lod.md](../bench/lod.md); evidence: `out/bench-runs/lod/`.

## 1. In one paragraph

A mesh of 64 triangles or more gets up to seven coarser levels when it is imported or generated,
each an index list over the mesh's own vertices with the geometric error it reached. Every level is
a row of the renderer's mesh table, so a level is drawn exactly like a mesh. Per instance and per
view, the culling pass takes the coarsest level whose error, scaled by the instance, stays under
one pixel under the instance's bounding sphere's perspective projection bound (the camera; a level
gets coarser only past a 20% margin) or under one texel (a shadow cascade). Levels are on by
default; `POCKET_LOD=off` draws every instance's full mesh, and so does a scene whose lists would
pass the device's binding limit with levels (5).

## 2. Making levels

`pocket_assets::lod::build(&mut MeshData, &LodOptions)`:

1. The full mesh's indices are ordered for the post-transform vertex cache
   (`meshopt::optimize_vertex_cache`). A mesh under `min_triangles` (64) gets no levels, only this
   order and the vertex order of step 5: a cube or a plane would cost a draw batch per level and
   save nothing.
2. Level `k` asks `meshopt::simplify` (meshoptimizer 0.25, `SimplifyOptions::ErrorAbsolute`) to
   bring the **full** mesh down to half of level `k - 1`'s triangles, with an error bound of half
   the bounding sphere's radius. Simplifying from the full mesh each time, not from the previous
   level, makes the reported error the level's own; a chain would have to add the errors up. The
   simplifier keeps attribute seams (vertices that share a position but not a normal or a texture
   coordinate) and borders by itself.
3. A level is kept only if it has at most 85% of the previous level's triangles; the chain stops at
   the first that does not (the error bound or the topology held the simplifier back), below 8
   triangles, or at 8 levels with the full mesh (`MAX_LEVELS`; the culling pass keeps a level in 4
   bits and reads at most 8 errors). Each level's indices are ordered for the vertex cache.
4. A level's error is never less than a finer level's (`error = max(error, reached)`): independent
   simplifications need not come out increasing, and selection (4) assumes they do.
5. The vertices are renumbered in the order the lists first use them, **coarsest level first**,
   then the finer ones and the full mesh (meshoptimizer's `optimizeVertexFetchRemap`, written in
   Rust because the crate's wrapper truncates the remap table to the used vertices, which loses the
   entries of unused ones). A coarse level then reads a short prefix of the vertex buffer. Vertices
   no list uses are dropped; skin weights move with their vertices; the bounds are recomputed.

Errors are in the mesh's own units (`Lod::error`, `MeshInfo::lod_error`). meshoptimizer's error is
its quadric metric's estimate of how far the simplified surface strays from the original, not a
proven bound (9).

**Where it runs.** On every mesh a glTF import produces (`import.rs`; natively on the asset
loader's worker thread, in the browser in the viewport's import of a fetched `.glb`), on the
renderer's seven primitives at `Renderer::new`, and on the demo models (`demo::mixed_model`,
`demo::lod_model`). The game module (`pocket-web`) and the game-side crates do not link it.

**Building it.** `meshopt =0.6.2` bundles meshoptimizer's C++ and compiles it with `cc`
(`pocket-assets` feature `lod`, which `import` enables and `pocket-render` enables for its
primitives):

- natively with clang-cl when `cargo xtask` runs the build (`CC_x86_64_pc_windows_msvc`, as for
  QuickJS-ng), otherwise with MSVC's `cl`, which ignores the workspace's `-ffp-contract=off` with a
  warning cargo hides for registry crates; on macOS and Linux with the system's C++ compiler;
- for `wasm32-unknown-unknown` with clang (`CC_wasm32_unknown_unknown`) and the stripped `assert.h`,
  `limits.h`, `math.h` and `string.h` the crate ships (`include_wasm32`, mapping `sqrtf`, `memcpy`
  and the like to compiler builtins), so no C or C++ library is linked. meshoptimizer's default
  allocator names `operator new(size_t)` and `operator delete(void*)` (`_Znwm`, `_ZdlPv`), which no
  library provides there: `pocket_assets::lod` defines both on wasm32 over Rust's allocator, each
  block carrying its size in a 16-byte header. Without them the link fails
  (`rust-lld: undefined symbol: _Znwm`); with them meshoptimizer adds no import to the module. On a
  Windows host the crate's build script runs its archiver as `llvm-ar` from `PATH`: `cargo xtask`
  puts the pinned LLVM's `bin` first on `PATH` there, and `tools/build_viewport.sh` finds the pinned
  LLVM (or `POCKET_LLVM`, or Homebrew's) and sets the compilers and `PATH`.

Levels made natively and in the browser can differ where the two compilers round meshoptimizer's
floats differently; they are render data and never reach the world or its hash.

## 3. Rows of the mesh table

`MeshPool::add` uploads the full mesh's indices followed by each level's into the shared index
buffer and pushes one `MeshInfo` row per level: the mesh's own row (the id instances name and the
scene counts) and, right after it, a row per coarser level with that level's `index_count` and
`first_index`, the same `base_vertex` and bounds, and its `lod_error`. The mesh's own row packs its
levels in `lods`: the count, the full mesh included, in the top 8 bits and the row of level 1 in the
low 24 (0: no coarser levels). `MeshInfo` stays 64 bytes: `lods` and `lod_error` replace two padding
words.

`MeshPool::owner[row]` is the row whose instances a row draws: itself for a mesh's row, the mesh's
row for a level row. A skinned copy (`add_dynamic`) gets level rows of its own over its own
vertices (the source's index ranges, its own `base_vertex`), so a skinned character's coarser
levels are skinned like its full mesh (the skinning pass writes all its vertices either way).

Other users of the table: ray-traced shadows build no bottom level for a level row (`is_level`;
shadow rays always meet full meshes, 7); CPU picking keeps a box per row; the selection outline
draws the full mesh.

## 4. Picking a level (cull.wgsl)

For an instance with `count > 1` levels (and levels on), with `s` its largest absolute scale, `c`
and `r` its bounding sphere's centre and radius in the world, and `e_k` level `k`'s error
(`e_0 = 0`), level `k` is **acceptable** in a view when `s * e_k <= B`, where `B` is:

- the camera, perspective: `d / sqrt(1 + (q / d)^2) * w`, with
  `w = pixels * 2 tan(fov_y / 2) / height` (`LodSettings::camera_terms`),
  `z = dot(c - eye, camera_forward)`, `d = max(z - r, 1e-4)`, and
  `q = length(c - eye - z * camera_forward) + r`. Axial depth sets perspective scale; the
  second factor bounds the extra screen displacement caused by a depth change off axis.
  Radial distance alone overestimates the permissible error near the edges of the view
  (found by the M5 integration test on 2026-10-10);
- the camera, orthographic: `pixels * view_height / height`;
- shadow cascade `i`: `shadow_texels * texel_i`, with `texel_i = 2 radius_i / 2048` the cascade's
  texel in metres (`shadows::Cascades::texel`). A cascade cannot show detail finer than its texels,
  and its level does not depend on the camera's distance, so it changes only when an instance moves
  between cascades: cascades need no hysteresis and usually draw coarser levels than the camera.

`coarsest(bound)` is the coarsest acceptable level. The camera's level is
`clamp(last, coarsest(B / (1 + h)), coarsest(B))`, `last` being the level the instance drew the
previous frame and `h` the hysteresis (0.2): a level gets coarser only once the coarser one would
fit the error bound divided by 1.2, and finer as soon as the current one is not acceptable. The
bound holds in every frame: the level drawn is always acceptable. A camera cut, a resize or a slot
reused by another entity can only change which acceptable level is drawn.

The camera's level is kept in bits 4 to 7 of the instance's state word (`state`, binding 7 of the
culling passes, occlusion.md 2). `main` writes it every frame levels are on (keeping `VIS_VISIBLE`
when occlusion culling is off), `late` keeps it, and with occlusion culling the late pass draws the
level the early pass chose. Turning levels off writes nothing (occlusion culling's own writes
store level 0).

Workgroup counting (cull.wgsl's two-level atomics) counts per view **and level** for the
workgroup's first instance's (variant, mesh): 40 workgroup counters (5 views x 8 levels) instead of
5; the late pass uses 8. The late pass's triangle counters count the level's triangles, so
occlusion culling's auto mode weighs the triangles it would really save.

## 5. Drawing, and what it costs

A batch stays (view, variant, row): a level row is a row like any other to `Batches`, the template,
the three draw paths and the id pass, so none of them changed. `Scene::batch_sizes` and
`batch_offsets` take the owner of each row: a level row's region in each view's list is sized by
its mesh's instances, because any of them may draw that level.

That is the price of this layout: an instance of a mesh with `L` levels has `L` entries in each
view's list, so the camera's `drawn` (48 bytes per entry) and the cascades' `visible` (4 bytes per
entry and view) grow by the levels: 68 bytes per instance and level (`Renderer::list_bytes`). With
levels off the level rows get no regions (the owner of a level row becomes none), and the lists
are as before. On the per-batch paths (the browser) every level that holds instances is a draw call
per view and variant, whether the culling kept any instance at that level or not; a skinned copy's
levels hold its one instance each, so every skinned part with `L` levels adds `L - 1` draw calls
per view there (docs/bench/lod.md 6).

**The binding limit.** Each list is bound whole, so it must fit the device's
`max_storage_buffer_binding_size` (and `max_buffer_size`); the lists grow by doubling up to that
limit, never past it. When the regions with levels would not fit, `sync` lays them out with levels
off instead and logs a warning: the scene draws full meshes, `FrameStats::lod` reads `off-limit`,
and the regions are laid out again (levels back on, if they fit) whenever the scene's counts
change. On WebGPU's default limits (128 MiB) that happens past about 350,000 instances of 8-level
meshes, or 400,000 of the 7-level primitive sphere. Before this fallback such a scene drew nothing:
the bind group failed validation, which wgpu only logs. Without levels the instance buffer (80
bytes per instance) reaches the limit first, at 1.68 million instances; nothing falls back there,
and an error is logged when the lists themselves do not fit.

The alternative removes the multiplication: count per (view, batch, level) in a first pass, place
each level's run with a small prefix sum, and write the entries in a second pass, the vertex stage
reading the baseline path's bases from storage. It costs a second pass over the instances and two
dispatches per phase; it is left until scenes need it (9).

## 6. Switches

| Setting | Natively | In the browser | Effect |
|---|---|---|---|
| mode | `POCKET_LOD=off` / `on` (default on) | `?lod=off` (viewport page) | off: every instance draws its full mesh; the level rows lose their regions |
| pixels | `POCKET_LOD_PIXELS` (default 1) | — | the camera's bound in output pixels; 0 draws full meshes on screen |
| shadow texels | `POCKET_LOD_SHADOW_TEXELS` (default 1) | — | the cascades' bound in their texels; 0 draws full meshes into them |
| hysteresis | `LodSettings::hysteresis` (0.2) | — | the margin a coarser level needs |

`Renderer::set_lod` and `set_lod_settings` change them at any time (from the next frame);
`FrameStats::lod` says whether the frame picked levels (`on`, `off`, or `off-limit` past the binding
limit, 5). `Renderer::draw_counts` reads back, per argument set and level, the instances and
triangles the last submitted frame drew (natively; it waits for the GPU), and
`Renderer::cascade_texels` gives the cascades' texels for a camera.

## 7. Interactions

- **Occlusion culling**: the early pass picks and stores the camera's level, the late pass draws it
  (4). Images are identical with occlusion culling off and forced on, cold and warm (8).
- **Ray-traced shadows** (`POCKET_RT_SHADOWS=1`) turn levels off. Their rays start a few millimetres
  off the drawn surface and meet the full meshes' bottom levels; a receiver drawn at a coarser level
  lies partly inside its full mesh and shadows itself. In `tests/rt_shadows.rs`'s static state the
  shadowed pixels' intersection over union with the cascades fell from 0.935 to 0.764 with levels on
  (2,927 ray-traced shadowed pixels against 2,386). Raising the ray origin by the level bound (the
  projected pixel at the point's distance, up to centimetres) brought 2,395 pixels but an overlap of
  0.879: contact shadows shrank by the offset. The instance's level error would give the exact
  offset, but the fragment stage does not know the level (the `Drawn` record has no free word). So
  the ray-traced renderer draws full meshes, and the check draws its comparison renderers with
  levels off too.
- **Shadow cascades** pick their own levels (4); a receiver's camera level and its caster level
  differ by at most a pixel's and a texel's error, within the cascades' normal offset of 1.5 texels
  where texels are coarser than pixels.
- **The id pass** draws the camera's batches, levels included, so picking and `render.visible` see
  what is drawn.
- **Skinned meshes** get levels made from the bind pose, drawn over the skinned vertices like the
  full mesh (3, checked in 8); an animated pose can stray further from the full mesh than the bind
  pose's error says.

## 8. Checks

- `pocket-assets` `lod::tests`: a chain halves (each level at most 85% and at least a quarter of the
  previous), its errors grow and stay within the bound, and indices stay in range; the coarsest
  level reads a prefix of the vertices and the full mesh uses every vertex; reordering keeps every
  triangle (by positions) and each vertex's skin weights; a cube gets no levels and the primitive
  sphere does.
- `crates/pocket-render/tests/lod.rs` (skips without a GPU):
  - `levels_cut_triangles_and_stay_within_a_pixel`: the LOD field (10 x 10 cells, rocks of 20,480
    triangles and knots of 12,288, 640x360) from two camera positions with levels off and on,
    through the same frames: with levels on the camera draws at most an eighth of the triangles over
    at least three levels, the cascades at most an eighth and some coarser level; every pixel whose
    entity differs is within two pixels of an edge of the full meshes' id image (an id
    discontinuity); fewer than 2% of the pixels differ by more than 32 of 255. Turning levels off
    again draws the full meshes' triangles exactly. The check compares against edges rather than
    against the other entity's nearest pixel: where two silhouettes that overlap with full meshes
    each shrink by a pixel, the levels show what was behind both, which the full meshes showed
    nowhere near.
  - `every_draw_path_and_occlusion_draw_the_same_levels`: the field with levels on, cold (the first
    frame: with occlusion culling every instance is drawn by the late pass) and warm, on the
    multi-draw path, with occlusion culling forced on, on the first-instance path and on the
    baseline path without and with occlusion culling: the same entity at every pixel, fewer than 1
    in 2,000 pixels apart by more than 8, and the same instances per level (with occlusion culling,
    none more per level).
  - `hysteresis_holds_a_level_until_the_margin`: one rock seen from 0.9, 1.1 (inside the margin),
    1.4, 1.1 and 0.9 times the distance where its level 1 becomes acceptable draws levels 0, 0, 1,
    1, 0; with occlusion culling off and on, and at scale 2.
  - `cascades_pick_levels_by_their_texels`: one shadow-casting rock seen from six distances (5 to
    130 m) that put it in every cascade, at scales 1 and 2 and with `shadow_texels` 1 and 2: every
    cascade that draws it draws the level computed on the CPU from `Renderer::cascade_texels` (54
    checks). The test also requires that its placements tell a cascade's level from the camera's
    and from a bound four times as loose.
  - `skinned_levels_follow_the_pose`: `demo::bent_model`, a skinned column of 5,120 triangles whose
    clip bends its upper half 90 degrees, seen far enough away that its copy draws level 2, with
    levels off and on: no pixel whose entity differs lies more than two pixels from an edge of the
    full mesh's id image. On the baseline path it pins the draw calls a skinned copy's levels add
    (5).
  - `levels_give_way_at_the_binding_limit`: on WebGPU's default limits, 419,429 spheres out of
    every view and 64 in it, just past the limit with levels: the frame draws exactly what levels
    off draws (`off-limit`), and levels come back once the hidden spheres are removed.
- The existing checks run with levels on: `draw_paths.rs` and `occlusion.rs` draw the primitives
  and the mixed model, whose spheres, capsules and tori have levels.
- `python tools/lod_mutations.py [--out file.json]` puts known bugs into the code one at a time,
  runs the checks against each and restores the file; it fails if any mutation survives. Results are
  in [docs/bench/lod.md](../bench/lod.md) 5.

## 9. Open

- The lists' multiplication by the levels (5): past the binding limit the whole scene falls back to
  full meshes. A cap per mesh would keep the levels of the meshes that fit; a two-pass placement
  would remove the multiplication.
- A skinned crowd on the per-batch paths: each copy's levels are draw calls of their own in every
  view, 8,165 calls instead of 1,025 for 200 skinned columns of 8 levels on the baseline path, 3.4
  ms more CPU encoding natively (docs/bench/lod.md 6). Fewer levels for skinned copies, or none on
  the per-batch paths, would trade their triangles for draw calls.
- Shading: a coarse level interpolates the full mesh's vertex normals over larger triangles, so
  its shading differs inside the silhouette (most of the field's differing pixels, docs/bench/lod.md
  3). `meshopt::simplify_with_attributes` weighing normals, or normal maps baked from the full mesh,
  would trade triangles for shading.
- Eight levels end at 1/128 of the triangles: the field's dense rocks (81,920 triangles) reach their
  coarsest level (640) within about 15 m at 720p, and most instances draw it. More levels, or
  impostors, would go further, each level multiplying the lists again.
- meshoptimizer's error is an estimate. The field check found no silhouette more than two pixels
  from where the full meshes put it, and a handful (3 and 14 of 230,400 pixels) between one and two.
- An intersection between two surfaces (a rock sunk into the ground) moves with both surfaces'
  errors and, at a grazing angle, further than either on screen.
- The browser imports a `.glb` on the page's main thread, levels included (docs/bench/lod.md 4).
- Ray-traced shadows draw full meshes (7).
- Mesh shaders and meshlets: an opt-in research spike, docs/bench/lod.md 7.
