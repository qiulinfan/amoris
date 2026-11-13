# Color grading

Status: Draft, Proposed. Implemented 2026-11-12 (charter 4.4's note of that date, 光照与画质).

Charter: 4.4 (光照与画质: HDR, bloom, AgX; the note of 2026-11-12 adds the grading table), 4.1 (one
renderer for Metal, Vulkan, Direct3D 12 and WebGPU), 3 item 7 (no silent limit: a table that fails
to load is reported).

This specification adds a 3D color lookup table ("LUT") to the display transform: the format it is
read from, where the world names it, how it reaches the renderer, where in the post chain it is
applied, what it costs, and how it is checked. The fog it is often paired with already exists
(`Environment.fog_density` and `fog_color`, exponential height fog in `forward.wgsl`); this file
does not change it.

The code is `crates/amoris-assets/src/lut.rs` (the table and its `.cube` reader),
`crates/amoris-assets/src/visual.rs` (`Environment`), `crates/amoris-assets/src/frame.rs`
(`EnvironmentView`), `crates/amoris-runtime/src/present.rs` and `engine.rs` (the feed and the
script schema), `crates/amoris-render/src/loader.rs` (loading), `post.rs` and `renderer.rs`
(`poll_grade`), `shaders/post.wgsl` (`graded`, `fs_tonemap_graded`) and
`crates/amoris-viewport/src/lib.rs` (the browser's loads).

## 1. The table and its file

A table is `size`^3 RGB entries; entry `r + size * (g + size * b)` (red changing fastest) is the
output at the input `(r, g, b) / (size - 1)`.

It is read from a `.cube` file, Adobe's Cube LUT Specification 1.0, which DaVinci Resolve, Adobe's
tools, Affinity and most grading tools write:

| Line | Read |
|---|---|
| `TITLE "..."`, blank lines, `# comments` | skipped |
| `LUT_3D_SIZE n` | required, `n` from 2 to 65 (17, 33, 64 and 65 are the usual sizes) |
| `DOMAIN_MIN 0 0 0`, `DOMAIN_MAX 1 1 1`, `LUT_3D_INPUT_RANGE 0 1` | accepted (the default domain); any other domain is refused |
| `LUT_1D_SIZE` | refused: only 3D tables |
| any other keyword | refused, with its line |
| `r g b` | an entry: three finite numbers; exactly `n`^3 of them, after the keywords |

The file is at most 16 MiB (a 65-entry table with nine decimals is about 8.8 MB). A refused file
names the line and the reason. Entries may lie outside [0, 1]; the output is clamped to [0, 1] when
it is written (the sRGB surface's decode clamps first; an 8-bit unorm target clamps on write).

**Color space.** A table maps display-encoded color to display-encoded color: its input is the
display transform's output after AgX (whose sRGB-like transfer is built in, post.wgsl `agx`), so a
table authored on an sRGB / Rec. 709 display-referred image in a grading tool (a "creative" or
"look" LUT) applies as it did there. It is not a scene-linear or log-encoded table: a camera's
log-to-Rec. 709 conversion LUT does not apply.

## 2. The data

`Environment` (persisted; `deny_unknown_fields`, defaults for every field) gains two fields, and
its version goes from 3 to 4:

| Field | Type | Default | Meaning |
|---|---|---|---|
| `color_lut` | string | `""` | a project-relative `.cube` path; empty for none |
| `color_lut_intensity` | f64 | 1.0 | the graded color's share of the output, 0 to 1 (clamped; 0 and non-finite values draw ungraded) |

A scene sets them in `scene.json` like the other environment fields
(`"Environment": {"color_lut": "grade/warm.cube", "color_lut_intensity": 0.8}`); a scene without
them keeps its frames. Scripts read and write both through the engine schema (`color_lut` a `str`
field, `color_lut_intensity` an `f64`; `ctx.world.set(env, "Environment", {...})`), and the editor's
inspector shows them as it shows every field of an engine schema. A script that fades the grade in
writes `color_lut_intensity`; a change of `color_lut` loads another table.

The render feed's `EnvironmentView` carries both (`color_lut: String`, `color_lut_intensity: f32`),
so the feed's `FORMAT` changes and the committed viewport (`web/viewport/pkg`) is rebuilt with it.

## 3. Loading

The renderer follows `EnvironmentView.color_lut` every frame (`Renderer::poll_grade`), as it follows
the baked GI asset: a new path drops the current table, so the frame draws ungraded until the new
one arrives, and asks the asset source for it (`AssetSource::request_color_lut`). Natively the
file loader reads it on its worker thread (`FileAssets`); in the browser the page fetches the path
and hands the bytes in (`PageAssets::deliver`), and `tools/build_web.sh` and `cargo xtask
export-web` copy `.cube` files with the project's other assets. A table in flight counts in
`FrameStats::pending_assets`. A table that fails to load (missing, malformed) is logged with its
reason, kept in `Renderer::color_lut_error`, and the frame draws ungraded; nothing is substituted.

## 4. The display transform

The post chain is unchanged up to AgX: bloom down and up, then the display transform reads the HDR
image, sharpens it (with TAA), adds bloom, applies exposure and AgX (docs/spec/taa-gtao.md). With a
table, `fs_tonemap_graded` then

1. reads the table at AgX's output `o`, clamped to [0, 1]: trilinear between its eight nearest
   entries, interpolated in the shader from `textureLoad` (so every GPU and API computes the same
   arithmetic, and the table can be `rgba32float`, which WebGPU does not filter);
2. blends: `mix(o, graded(o), color_lut_intensity)`;
3. dithers and encodes as before (`encoded`: the per-pixel `pcg_hash` dither, and the sRGB decode
   when the surface encodes).

The table is applied before the dither, so a smooth table keeps the dither's protection against
banding. `amoris_assets::lut::ColorLut::sample` is the same interpolation on the CPU (the GPU test's
expected colors).

**Off by default.** With no table (`color_lut` empty, a table not yet loaded or failed) or a share
of 0, the display transform is `fs_tonemap`, the entry point every frame used before, and its
parameters are those of before (the share's slot, `exposure.w`, is written 0): the frame is the same
to the bit. The graded pipeline is built when the first table arrives, so a renderer that never
grades never compiles it.

**Resources.** The table is a 3D `rgba32float` texture, `size`^3 texels (33: 575 KB; 65: 4.4 MB).
`fs_tonemap_graded` binds it at binding 4 of the post shader's only group, beside the uniform, the
image, the sampler and the bloom chain: one more sampled texture, no storage buffer and no sampler,
well inside WebGPU's per-stage limits. Nothing else in the frame changes.

**WebGPU baseline.** The table needs no optional feature or limit: `rgba32float` 3D textures read
with `textureLoad` (unfilterable float, the auto layout's choice when no sampler samples them) are
core WebGPU, and the largest table (65) is far below `maxTextureDimension3D` (2048). It runs under
`AMORIS_GPU_MINIMAL=features,limits,timestamps` and in the browser unchanged.

**Cost.** Eight texel loads and seven lerps a pixel in the display transform's fragment shader, only
while a table is graded; no new pass.

## 5. What is not built

- 1D tables, shaper LUTs and non-default domains (`DOMAIN_MIN`/`DOMAIN_MAX`): refused, not
  approximated.
- Tables applied in scene-linear or log space, before tone mapping (where a "technical" grade
  would sit): the table sits after AgX only.
- Strip or atlas PNG tables (Unreal's and Unity's 2D layout): `.cube` only.
- Blending two tables over time (a script changes `color_lut_intensity`, or swaps `color_lut`,
  which draws ungraded until the new table loads).
- The path tracer and the GI bakers ignore the table: it is a display transform, not light.

## 6. Checks

- `cargo test -p amoris-assets lut`: the `.cube` reader (red-fastest order, the round trip through
  `to_cube`, the CPU interpolation, every refusal with its line).
- `cargo test -p amoris-runtime --lib engine`: the environment's script slots round-trip both
  fields.
- `cargo test --release -p amoris-render --test color_grade` (also on D3D12 and Vulkan on both
  GPUs, and under `AMORIS_GPU_MINIMAL`): a mixed scene drawn into an sRGB-encoding and a plain 8-bit
  output, graded by tables the asset source hands in as the environment names them. No table, a
  table at share 0, a cleared table and a table that fails to load leave the frame unchanged to the
  bit; the identity at 33 and 17 entries changes at most 0.1% of the channels by at most one 8-bit
  step; a channel swap matches the ungraded frame's channels swapped within one step; a sepia
  matrix matches `ColorLut::sample` of the ungraded frame within two steps; half an inversion is mid
  grey within one and a half steps.
- `tools/render/build_compare.py` against a build without the table: every case and mode the same
  to the pixel (the off path).
- `tests/fixtures/scenes/color-grade` (written by its `build_scene.py`: a coloured row and a grey
  ramp, graded by `grade/warm.cube`; `scene-off.json` without the table): `showcase_bench` draws
  it natively, and `tools/build_web.sh` packs it for the viewport page in Chrome.
