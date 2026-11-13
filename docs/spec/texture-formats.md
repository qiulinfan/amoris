# Texture formats: KTX2 and block compression

Status: Draft, Proposed. Implemented 2026-11-12 (charter 4.4's note of that date, 资源与纹理).

Charter: 4.1 (资源: glTF 2.0, PNG/JPEG and, from 2026-11-12, KTX2), 4.4 (one renderer for Metal,
Vulkan, Direct3D 12 and WebGPU; the WebGPU baseline's optional features are acceleration paths),
3 item 7 (no silent limit: a file that does not read is refused with its reason).

This specification adds KTX2 textures to the glTF importer and block-compressed texture layers to
the renderer: which files are read, how glTF names them, what the importer hands the renderer,
where each image's layer comes from, what a device without block compression does, how the
fixtures are made, and how it is checked. Before it, images were PNG or JPEG decoded to RGBA8 and
every texture layer was RGBA8, resized and mipmapped on the GPU (materials.rs, blit.rs).

The code is `crates/amoris-assets/src/ktx2.rs` (reading, decoding, writing),
`crates/amoris-assets/src/mesh.rs` (`ImageData::levels`, `TextureLevels`, `BlockFormat`),
`crates/amoris-assets/src/import.rs` (`load_images`), `crates/amoris-render/src/materials.rs`
(`placement`, the BC7 arrays), `blit.rs` (`write_levels`), `gpu.rs`
(`Capabilities::texture_compression_bc`, `Minimal`'s `texture-compression`), the textures group in
`renderer.rs` and `shaders/forward.wgsl` (`color_grad`, `data_grad`, `color_level`); the fixture
tool is `crates/amoris-render/examples/ktx2/`.

## 1. In one paragraph

A glTF texture may name a KTX2 image, through `KHR_texture_basisu` or as its plain source. The
importer reads it with its levels (Zstd supercompression undone), decodes level 0 to RGBA8 on the
CPU for the readers that want texels, and keeps the levels. On a device that samples BC
(`texture-compression-bc`: desktop GPUs natively, desktop Chrome), a BC7 image whose levels fit a
texture layer is uploaded as stored into a BC7 texture array, a quarter of the RGBA8 layer's memory,
with no decoding and no mip generation; any other KTX2 image, and every KTX2 image on a device
without BC, is decoded to RGBA8 on the CPU, its own levels written where they fit. Nothing changes
for a model without KTX2 images: no array is made, the frames are the same to the bit.

## 2. Files read

`amoris_assets::ktx2::read` takes a KTX 2.0 file of one 2D image:

| | Read | Refused (with the reason) |
|---|---|---|
| Format (`vkFormat`) | `R8G8B8A8_UNORM`/`_SRGB`; `BC1_RGB(A)_UNORM`/`_SRGB`; `BC3_UNORM`/`_SRGB`; `BC4_UNORM`; `BC5_UNORM`; `BC7_UNORM`/`_SRGB` | undefined (Basis Universal: BasisLZ/ETC1S and UASTC), ETC2, ASTC, BC2, BC6H, signed BC4/BC5, every other format |
| Supercompression | none, Zstandard (scheme 2) | BasisLZ, ZLIB |
| Shape | width and height at most 16384; `levelCount` 0 (one level) to a full chain | 1D, 3D, array layers, cube faces |
| Levels | level `k` is `max(1, width >> k)` by `max(1, height >> k)` texels, whole 4 x 4 blocks for BCn | a level whose size (after Zstd) is not that |

The file's sRGB or linear variant is read but does not decide anything: glTF's use of the image
does (base color and emissive are sRGB-encoded, normals, metallic-roughness, occlusion and
transmission linear), as it does for PNG. BC1 is read as WebGPU's only BC1, `bc1-rgba-unorm`: a
block in 3-color mode has a transparent black fourth color, also for a `BC1_RGB` file.

**Decoding** (`ktx2::decode`, `texture2ddecoder`): every format to RGBA8 texels. BC4 decodes to
grey `(r, r, r, 1)` and BC5 to `(r, g, 0, 1)`, as the importer reads one- and two-channel PNGs; a
two-channel normal map's z is not rebuilt (7). The decoder is pure Rust and runs in the browser.

**Writing** (`ktx2::write`): the same formats, levels smallest first with KTX 2.0's alignment, a
basic data format descriptor (its plane sizes 0 when supercompressed) and a `KTXwriter` entry;
Zstandard at its fastest level (`ruzstd`'s encoder). The fixture tool and the tests write files
with it (6).

## 3. Where KTX2 enters: glTF

A texture's image is chosen in this order (`import.rs`, `load_images`):

1. the `source` of its `KHR_texture_basisu` extension, when that KTX2 image reads;
2. its own `source` (the extension's fallback, usually a PNG), when that reads;
3. none: the slot draws without a texture.

A KTX2 image that does not read (a Basis Universal file, say) falls back to the texture's `source`;
without one the import fails with the reason (`texture 0's KTX2 image 1 does not read: a Basis
Universal KTX2 ... needs transcoding`). An image only a working KTX2 image's fallback names is not
decoded (its slot in `ModelAsset::images` is an empty placeholder, so the file's indices hold).
A plain `source` may be a KTX2 image too: by its `mimeType` `image/ktx2`, a `.ktx2` URI, or the
KTX2 identifier at the start of its buffer view. Images may be buffer views (a `.glb`), files beside
a `.gltf` (percent-decoded URIs) or base64 data URIs. `KHR_texture_basisu` may be in
`extensionsRequired`: the `gltf` crate allows a texture without `source` under it (its
`allow_empty_texture` feature) and the importer reads the extension through its `extensions`
feature.

There is no other texture slot: a `Model` names a mesh and a glTF material, never an image, so KTX2
reaches the renderer only through glTF files (natively `import_gltf`, in the browser
`import_glb_bytes`). Packaging needs nothing new: `tools/build_web.sh` and `cargo xtask export-web`
already copy `.ktx2` files, and export-web's `.gltf` to `.glb` conversion gives a `.ktx2` URI the
media type `image/ktx2`.

The imported image (`ImageData`) holds `rgba8`, level 0 decoded, and `levels:
Some(TextureLevels { format, levels })`, the file's levels as stored. The CPU's readers (the path
tracer, the GI bakers, the neural texture encoder) read `rgba8` as they read a PNG's.

## 4. Texture layers

Every material's textures are layers of texture arrays bound in the textures group (group 2), each
layer [`TEX_SIZE`] (1024) square with its full chain of 11 levels: one array for sRGB-encoded and
one for linear data, RGBA8, as before; and now, made at the first image that takes one, a BC7 sRGB
and a BC7 linear array (bindings 6 and 7; until then a 1-texel RGBA8 stand-in is bound there). A
layer index with bit 30 set (`TEX_BLOCKS`) names a layer of the BC7 array of its kind; the forward
shader's `color_grad`, `data_grad` and `color_level` (also used by the ray-traced shadows and
reflections) pick the array by that bit.

An image's levels **fit** a layer when it is square, its side a power of two of at least 1024, and
the file has every level from the one of 1024 down to 1 x 1 (`materials::fitting_level`; a 2048
image with its chain starts at its level 1). Never for an image marked for point sampling
(`nearest`, voxels.md 7.2). Then (`materials::placement`):

| Image | Device with BC | Device without BC |
|---|---|---|
| BC7, levels fit | **blocks**: the file's levels written as stored to a BC7 layer (`blit::write_levels`) | **levels**: each level decoded on the CPU and written to its mip of an RGBA8 layer |
| RGBA8, BC1, BC3, BC4 or BC5, levels fit | levels | levels |
| any other KTX2 image (smaller, not square, a short chain) and every PNG or JPEG | **resize**: level 0 (`rgba8`) drawn into the layer by the linear sampler and its mips drawn from it on the GPU, as before | resize |

Only BC7 goes to the GPU as stored: two more texture bindings are what the textures group can
spend (6), and BC7 is the format that keeps color, alpha and data maps at one quality; BC1 to BC5
images are decoded, so every device draws them alike. Layers stay 1024: a BC7 layer with its chain
takes 1.4 MB against the RGBA8 layer's 5.6 MB. A BC7 array grows as the RGBA8 ones do, by doubling
and copying (a level smaller than a block copies whole blocks).

**Decoding costs.** The importer reads every KTX2 image (Zstd undone) and decodes its level 0 on
its worker thread natively, on the page's thread in the browser; the levels path decodes levels 1
to 10 when the model's materials are added, on the render thread. For a 1024 x 1024 image with its
chain (the `ktx2` example's `encode` prints these; R1, release build, the fastest of five runs
while other builds and GPU runs loaded the machine): BC7 read 8.0 ms, level 0 decoded 37 ms,
levels 1 to 10 12 ms; BC1 4.6, 10 and 2.7 ms; RGBA8 6.7, 0.9 and 0.4 ms. The blocks path decodes
no level on the render thread.

## 5. Off by default; the WebGPU baseline

A model without KTX2 images makes no BC7 array, writes the same layers by the same draws, and
leaves `TEX_BLOCKS` clear in every material row, so the shader takes the RGBA8 arrays as before:
the frames are the same to the bit (8). The stand-in costs one texel.

`texture-compression-bc` is an optional WebGPU feature (webgpu-baseline.md 1): `Gpu::new` asks
for it when the adapter offers it (`Capabilities::texture_compression_bc`); without it
(`AMORIS_GPU_MINIMAL=features`, or `texture-compression` alone, natively; `?gpu_minimal=` in the
browser; phones and Apple GPUs without BC; WebGPU's defaults) every KTX2 image takes the levels or
resize path and draws within one 8-bit step of the BC7 layer (8). The two new bindings are sampled
textures, not storage buffers: the forward pipeline's fragment stage stays at 8 of 8 storage
buffers, and its sampled textures stay within WebGPU's default 16 (the textures group's five, the
lighting group's three, and on the water pipelines group 3's).

`FrameStats::texture_blocks` (the viewport's stats: `texture_blocks`) counts the layers held as
BC7.

## 6. Making files

`cargo run --release -p amoris-render --example ktx2 -- encode IN.png OUT.ktx2 [--format
bc7|bc1|bc3|bc4|bc5|rgba8] [--srgb] [--no-mips] [--no-zstd]` writes a PNG as a KTX2 file: its full
chain of 2 x 2 box averages, each level encoded by Intel's ISPC texture compressor (`intel_tex_2`,
native only, so the tool is an example), Zstd-supercompressed. Other tools' files read as long as
they are in a format of section 2 (KTX-Software's `toktx --encode` makes Basis Universal files,
which are refused; `ktx create --format BC7_SRGB_BLOCK --zstd` makes readable ones).

`tests/fixtures/scenes/ktx2/` is a scene of four panels whose model, `models/panels.glb` (70 KB),
the same example writes (`fixture DIR`), run by the folder's `build_scene.py`: `bc7` (1024 BC7 base
color and normal map: blocks), `bc1` (1024 BC1: levels), `small` (256 BC7: resize) and `png` (the
same pattern as `small` as a PNG). `tools/build_web.sh tests/fixtures/scenes/ktx2` packs it for
the browser.

## 7. What is not built

- **Basis Universal** (ETC1S/BasisLZ and UASTC, what `KHR_texture_basisu` was written for): it
  needs the Basis transcoder, C++ (`basis-universal` crate) that would have to build into the
  browser's game and viewport modules with the wasm toolchain, plus a choice of target format per
  device. Refused with the reason; the texture falls back to its PNG when it has one. The next step
  once wanted: transcode UASTC to BC7 (desktop) or ASTC (phones) at import.
- **ASTC and ETC2** files and their GPU paths (Apple GPUs without BC, phones): one block format per
  array pair, and no device here to check them; with Basis above they would be its other targets.
- BC1 to BC5 sampled as stored: each would need two more texture bindings, or BC7 transcoding.
- Smaller-than-layer images as stored: a 512 BC7 image is decoded and resized (layers are 1024).
- ZLIB supercompression, BC6H (HDR), arrays, cube maps and 3D textures.
- Rebuilding a two-channel normal map's z (BC5, RG PNGs alike).

## 8. Checks

- `cargo test -p amoris-assets ktx2 import`: writing and reading RGBA8 and BC levels plain and with
  Zstd, the blocks' decoded colors and channels (BC1's 3-color mode, BC4 grey, BC5), every
  refusal; glTF: a `KHR_texture_basisu` texture takes its KTX2 image (sRGB by use, its levels, its
  PNG fallback left undecoded), a plain `image/ktx2` source, a Basis image falling back to its PNG
  or failing the import without one, a `.gltf` reading a `.ktx2` file beside it.
- `cargo test --release -p amoris-render --test ktx2` (D3D12 and Vulkan on the RTX 5060 and the
  Radeon 780M, and under `AMORIS_GPU_MINIMAL=features,limits,timestamps`):
  `the_cpu_decodes_each_bc_format_as_the_gpu_samples_it` uploads BC1, BC3, BC4, BC5 and BC7 chains
  (128 to 1) and compares every texel the GPU loads with the decoder's: BC7 exactly, BC4 within
  two 8-bit steps, BC5 within three, BC1 and BC3 within four (the decoder truncates its
  interpolated thirds and sevenths; measured 2026-11-12: BC7 0 everywhere, BC1 and BC3 4 on the
  RTX 5060 and 2 on the 780M, BC4 2 on both, BC5 3 on the 5060 and 2 on the 780M); skipped without
  BC.
  `a_ktx2_textured_model_draws_alike_with_and_without_bc` draws the four placements close up and
  from afar on a device with BC and one without: the frames within one 8-bit step, close up also
  the decoded images handed in as PNGs are (measured: no pixel differed in any view on the four
  configurations); two BC7 layers counted with BC, none without.
- `cargo test -p amoris-render --lib materials`: `placement` for each kind of image.
- `tools/render/build_compare.py` against a build without KTX2 (321bb008; every case and mode,
  pt-lab's textured and alpha-masked cards among them, D3D12 and Vulkan on both GPUs): 120 pairs,
  the same to the pixel.
- The browser: `tools/build_web.sh tests/fixtures/scenes/ktx2`, then `node tools/web_bench.mjs` on
  the game page (`/?package=projects/tests-fixtures-scenes-ktx2/package.json`) with and without
  `&gpu_minimal=features` (or `texture-compression`) in headless Chrome: Chrome 154 (D3D12, R1)
  offers `texture-compression-bc`; `texture_blocks` 2 with it and 0 without, the panels drawn, and
  the canvases of the three runs the same to the pixel.
