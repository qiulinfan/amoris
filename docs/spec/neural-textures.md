# Neural texture compression

Status: implemented 2026-10-09 (Pioneer), branch `explore/neural`; charter 4.4 and its note of the
same date (training runs in the engine's wgpu compute, not Python/PyTorch). Measurements:
[bench/neural-textures.md](../bench/neural-textures.md). The lost branch `feat/neural` (schedule
3.3) left no code; this is a new implementation, compared with its reported numbers where they
exist.

A material's textures (base color, normal, occlusion/roughness/metallic, emissive, height: up to
twelve channels) are stored as two quantized latent grids per pair of mips plus one small MLP that
decodes any texel of any mip; the forward pass's fragment shader runs the MLP per pixel. The
encoder trains a file on the GPU with the engine's own compute shaders, on every native backend.

| Piece | Where |
|---|---|
| File format, validation, CPU reference decoder | `crates/pocket-assets/src/neural.rs` (game side, wasm-safe) |
| GPU table, packing, whole-mip decode for reports | `crates/pocket-render/src/neural/mod.rs` |
| Trainer | `crates/pocket-render/src/neural/train.rs`, `shaders/neural_train.wgsl` |
| Runtime decoder | `shaders/neural_texture.wgsl`, `neural_mlp.wgsl` (generated), `neural_f16.wgsl`, `neural_f32.wgsl` |
| Forward-pass variant | `shaders/forward_neural.wgsl`, renderer.rs, materials.rs |
| Encoder and report | `crates/pocket-render/examples/neural_encode/` |
| Decode cost | `crates/pocket-render/examples/neural_bench.rs`, the viewport's `?demo=neural` |
| Tests | `tests/neural.rs`, `tests/neural_material.rs`, `tests/neural_mlp.rs`, unit tests in both crates |

## 1. Using it

```text
cargo run --release -p pocket-render --example neural_encode -- --out OUT procedural:bricks
cargo run --release -p pocket-render --example neural_encode -- --out OUT "gltf:samples/pt-lab/materials.gltf#CheckerPBR"
```

writes `OUT/<name>.ntex`, a JSON report and a preview strip. A `Model` whose `material` is the
project-relative path of a `.ntex` (`materials/bricks.ntex`) draws with it; its `color` and
`emissive` multiply and add as for any asset material. Nothing in the render feed changed: the
material is an asset path like `models/boat.glb#Sail`.

## 2. The file (`.ntex`, version 1)

Little-endian. Every size is derived from the header and must match; anything else is refused.

| Bytes | Field |
|---|---|
| 4 | magic `NTEX` |
| 4 | version, 1 |
| 4, 4, 4 | width, height (powers of two, at most 4096), mip count (1 to the full chain) |
| 4 + 16 | channel count (1 to 16), then a code per channel, unused slots 0xff |
| 4 x 4 | fine grid features and bits, coarse grid features and bits |
| 4 x 5 | fine shift (1 to 3), sampling (0 bilinear, 1 four taps), positional-encoding octaves (0 to 3), two hidden widths (multiples of 4, at most 64) |
| 4 + n | name length (at most 256) and UTF-8 name |
| 4 | latent level count, `ceil(mips / 2)` |
| per level | fine width, height, coarse width, height (4 x 4), then each grid's words |
| 4 + 2 n | weight count, then the network as IEEE halves, padded to 4 bytes with zero |

Channel codes are stable and appear in ascending order: 0 to 2 base color (sRGB-encoded, as glTF
stores it), 3 and 4 tangent-space normal x and y (`n * 0.5 + 0.5`, z reconstructed), 5 to 7
occlusion, roughness, metallic (glTF's ORM, linear), 8 to 10 emissive (sRGB-encoded), 11 height
(linear; carried, not shaded). Base color, normal and emissive need all their channels or none.

A latent texel is 64 bits in both grids: 8 features of 8 bits or 16 of 4, low bits first,
dequantized to `q / (2^bits - 1)`. Level `l` serves mips `2l` and `2l + 1`; its fine grid is mip
`2l`'s size shifted right by the fine shift (2 by default: a quarter of the resolution), its coarse
grid half that, each at least 1x1. Grids are row-major.

The network has two ReLU hidden layers and a linear output. Weights are layer by layer, each a
row-major `outputs x inputs` matrix then the biases, with the input count padded to a multiple of
4 and the output count likewise; padding weights must be zero.

## 3. Decoding

### 3.1 The reference

`pocket_assets::neural::Decoder` defines the decode; the GPU decoder must agree with it
(`tests/neural.rs`: within 5e-7 in f32, 2.5e-3 in f16, every texel of every mip). For mip `m` at
`uv` (wrapped into [0, 1): neural textures always repeat):

1. `l = m / 2`. For each grid of level `l`, `p = uv * size - 0.5`; the taps are `floor(p)` and
   `floor(p) + 1` on both axes, wrapping; the bilinear weights are those of `fract(p)`.
2. The input vector: the fine grid's features, blended (`Bilinear`, 8 or 16 inputs) or the four
   taps concatenated, x fastest (`Taps4`, 32 or 64); the coarse grid's features, blended; for each
   octave `k` and axis, `tri(p * 2^k)` and `tri(p * 2^k + 0.25)` of the fine grid's `p`, where
   `tri(t) = |2 fract(t) - 1|`; then `m & 1` and `l / 8`; zeros to a multiple of four.
3. Two dense ReLU layers, a linear output; consumers clamp each channel to [0, 1].

The positional encoding tells the network where the pixel lies between latent texels (a
quarter-resolution grid covers four texels of mip `2l` per latent texel); the quarter-phase pair
makes each phase distinct. The level-of-detail inputs let one network serve every mip.

### 3.2 On the GPU

`NeuralTable` (neural/mod.rs) holds every loaded texture: latent texels in one `rg32uint` texture,
4096 texels a row, each texture's grids from a row's start, in level order (fine, coarse); and one
64 KiB uniform (`array<vec4u, 4096>`, WebGPU's default binding limit) with, per texture, a
20-word descriptor (size and mip count; where each channel group is; per level the two grids' size
and first texel) and the weights in block order: per layer, for each block of 4 outputs and 4
inputs, the 4x4 halves column by column in two words, then a word of 4 biases per output block,
the bias words padded to an even count so every block starts on an even word. The forward pass
already binds WebGPU's default of 8 storage buffers per stage, so the decoder uses a texture and a
uniform, not storage.

A texture's words take about `20 + 2 * blocks + biases`: 320 words (5 KiB) for the default
26-32-32-12 network, so the uniform holds 12 such textures; the latent texture grows by doubling
its rows up to the device's 2D limit (8192 rows on WebGPU: 256 MiB of latents).

The network's shape is compiled into the shader as constants (`NT_*`, `neural::profile_constants`).
The first texture a renderer loads fixes the profile its neural pipelines are compiled for. The
forward pass finds each texture's channel groups in its descriptor, so textures share a profile when
their grids, sampling, encoding and hidden widths match and their channel counts pad to the same
number of four-vectors (9 to 12 channels share; 8 do not). A texture of another shape is refused
and its material falls back to the default.

Precision: with `shader-f16` the network runs in half precision (`neural_f16.wgsl`); the weights
are read as halves through a second, half-typed view of the same uniform (`array<NtBlock, 2048>`,
four `vec4<f16>` per block) because naga cannot bitcast a `u32` to two halves. Without it
(`neural_f32.wgsl`) the halves are widened with `unpack2x16float`. `POCKET_NEURAL_PRECISION=f32` or
`Renderer::set_neural_half_precision(false)` forces single precision.

### 3.3 Why the network is generated unrolled

The first decoder wrote the layers as loops to constants. naga guards every loop with a hidden
64-bit counter (`force_loop_bounding`, one of its runtime checks), and the drivers then kept the
loops, with the input and hidden activations in local memory: on the RTX 5060 a 1920x1080 decode
cost 8.9 ms in f16 and 17.3 ms in f32. Unrolled it costs 2.4 and 3.4 ms (bench, provisional).
Turning the check off (`create_shader_module_trusted`, as the online NRC's lean option does) was
not pursued for the renderer's modules. Instead `neural_mlp.wgsl` is generated by
`tests/neural_mlp.rs` (`POCKET_BLESS=1` rewrites it; the test fails when it is stale, the pattern
of pocket-script's prelude): a statement per 4x4 block up to the largest profile the format allows
(24 input blocks, 16 hidden, 4 output), each behind a constant condition on `NT_*` that drivers
fold, with indices clamped to constants valid in every profile. `nt_inputs` is written out by hand
the same way. One file serves every profile.

### 3.4 One decode call, its weights located per call

Within the level-of-detail blend band (section 5) a pixel decodes twice, with the same weights. Two
arrangements were slow (bench 5.2): a call per branch put four copies of the unrolled network in the
shader, which overflowed the instruction caches where neighbouring pixels took different branches
(4.5 ms instead of 1.4 on the RTX 5060 for a view across the ground); and any form whose two decodes
loaded the same weights from the same addresses let compilers share or hoist those loads, holding
thousands of weights in registers and spilling (the Radeon 780M up to 56 times slower on Direct3D
12, and a two-call form even the 5060 ten times slower). `fs_neural` calls `nt_decode` once in a
loop of one or two iterations, and each iteration reads where the weights start from its own
descriptor word (2 or 3, both holding the same value): no compiler can prove the two equal, so each
decode's loads stay inside it. `nt_decode` takes that word as its `weights` argument.

## 4. Training

`Trainer` (train.rs, neural_train.wgsl) trains latents and network together by Adam on an L2 loss,
everything on the GPU: no data returns to the CPU until the end.

- **Reference.** Mip 0 is the material's channels in [0, 1] as stored. Further mips are 2x2 box
  filters: color in linear light, normals as vectors renormalized, the rest as stored.
- **Samples.** Each step draws `batch` samples (65,536). The mip is drawn with probability
  proportional to `0.5^m`; the position is a texel centre for a share of 0.75, anywhere otherwise,
  against the mip's bilinearly filtered reference (so magnification between texels is trained
  too). Random numbers are PCG hashes of (seed, step, sample).
- **Quantization.** The latents are continuous in [0, 1] while training. For the first 60% of the
  steps the forward pass adds uniform noise of one quantization step; then it rounds, with a
  straight-through gradient; from 85% the latents are frozen at their rounded values and only the
  network trains. The file stores `round(clamp(z) * (2^bits - 1))`, ties to even (WGSL's `round`).
  The network is rounded to halves at the end (its f16 loss is measured, not trained).
- **One step, five dispatches.** `forward_backward` (a sample per invocation: input, forward,
  loss, backward through the network, the latent gradients); `mlp_partials` (a parameter per
  invocation and a chunk of 256 samples per workgroup row: the gradient summed over the chunk from
  the per-sample records of activations and deltas); `mlp_adam` (sums the chunks in order, Adam);
  `latent_adam` (Adam per latent value, clamped to [0, 1]); `finish_step` (the mean loss into the
  history, the step counter). Steps are recorded 20 to a command buffer.
- **No float atomics, deterministic.** A latent receives gradient from every sample whose taps
  touch it. Each contribution is scaled by 2^14, clamped to +-32 and stochastically rounded to an
  integer (with the hash generator), then added with `atomicAdd` on `i32`: integer sums do not
  depend on the order. The network's gradient is summed in a fixed order. A seed reproduces the
  file bit for bit on one device and backend (`tests/neural.rs`; a float compare-exchange add put
  back in its place fails that test).
- **Initialization.** Latents uniform in [0.25, 0.75], hidden layers He-uniform, the output layer
  a tenth of that with its biases at the channels' means; all from xorshift of the seed.
- **Defaults** (`TrainConfig`): 10,000 steps, network learning rate 0.005 and latent 0.03, cosine
  decay to 5%, Adam (0.9, 0.999). They came from the sweeps in the bench (section 4 there).

The loops of the training shader are not unrolled (it runs offline): about 3.2 ms a step on the
RTX 5060, half a minute a material.

## 5. In the renderer

- **Loading.** `Pools::parts` sees a look whose material ends in `.ntex` (`is_neural_texture`, which
  compares the suffix as bytes: every look's material passes it, and names such as `金属` or
  `models/x.glb#屋顶` once panicked when the string was sliced inside a character); until the
  material is registered the entity waits, as for a model. Natively `FileAssets` loads the file on
  its worker thread (`NeuralTexture::load`); in the browser the page fetches it like a model and
  `PageAssets::deliver` parses it. `Renderer::add_neural_texture` uploads it into the table, builds
  the neural pipelines on the first texture, and registers a material row: flag 8 (neural), the
  descriptor word in the row's former padding word, factors 1 for the channels the texture has
  (roughness 0.5 and metallic 0 when it lacks them). A file that fails to load or does not fit
  registers the default material under its path, with an error in the log.
- **Variants.** Pipeline variants grow from two bits to three (`VARIANT_MASK`, `VARIANTS = 8`):
  bit 2 is the neural variant. Its forward pipelines use a module of their own,
  `shaders::forward_neural`: forward.wgsl, the decoder and forward_neural.wgsl's `fs_neural` (traced
  shadows included when they are on). The other variants keep forward.wgsl's module, which contains
  no decoder, so ordinary materials compile and run as before; on the multi-draw path a neural
  variant with no instances issues no draw (`tests/neural_material.rs` checks a frame is
  bit-identical with an unused neural texture loaded). Neural materials are opaque (no alpha
  channel) and single-sided; their shadows use the opaque shadow pipeline.
- **Level of detail.** `fs_neural` takes the isotropic level of hardware filtering from the UV
  derivatives in texels of mip 0 (the longer axis), clamped to the mip chain. It decodes one mip,
  or, within a quarter of a level around the transition from mip `m` to `m + 1`, both and blends
  them ("brilinear"): one network evaluation for three quarters of the pixels. There is no
  anisotropic filtering: at grazing angles the neural material is blurrier than the texture arrays,
  which sample with 16x anisotropy.
- **Shading.** Base color and emissive are decoded from sRGB, normals rebuilt with z, and the
  shared helpers of forward.wgsl (`shading_normal`, `antialiased_roughness`) do the rest; occlusion
  became a `Surface` field that scales the sky's diffuse and specular light (1 for ordinary
  materials, whose occlusion textures the renderer still ignores).

## 6. How quality and size are reported

The encoder decodes every texel centre of every mip through the runtime decoder on the GPU, in
f32 and f16, and compares with the reference quantized to 8 bits (what an uncompressed texture
stores). PSNR is per channel group over its channels, in [0, 1] units; normals also get the mean
angle between decoded and reference vectors. Bits per texel count the latents and the weights of
every mip against mip 0's texel count (the header and level sizes, about 200 bytes, are left out).

Two BCn baselines encode each group at every mip with Intel's ISPC compressor (`intel_tex_2`,
BC7 "basic" quality) and decode it with `texture2ddecoder`: high is BC7 for base color, ORM and
emissive, BC5 for normals, BC4 for height; low is BC1 instead of BC7. The ORM group keeps glTF's
slots (occlusion, roughness, metallic in R, G, B). "Half resolution" is the high set encoded at
mip 1 and bilinearly upsampled to mip 0, a quarter of the bits. Uncompressed is 8 bits per channel
(and, as GPU formats need, RGBA8 for three-channel groups).

## 7. Tests

| Test | Checks |
|---|---|
| `pocket-assets` `neural::tests` | f16 conversion over all 65,536 values; layout sizes and padding; byte round trip; malformed files refused (truncation, trailing byte, magic, version, size, duplicate or unknown channel, latent bits, partial group, nonzero padding, NaN weight, grid size, mip count); the input vector and a hand-evaluated network |
| `pocket-render` `loader::tests` | `.ntex` paths are recognized in any case and script; names that are not ASCII do not panic |
| `pocket-render` `neural::tests` | weight block packing; profile constants; every shader composition validates in naga (both samplings, f16 and f32, traced or not); mip building; quantization rounding |
| `tests/neural.rs` | training lowers the loss and reaches 39 dB on a test pattern in 600 steps; GPU decode equals the CPU reference (f32, f16); a seed reproduces the file |
| `tests/neural_material.rs` | a constant network draws within 1/255 of the equivalent inline material in f16 and f32, on WebGPU's baseline (no optional features, default limits), and as the second of two textures with 9 and 12 channels; an 8-channel texture is refused beside them; an unused neural texture leaves a frame bit-identical. A probe network that ignores the position and outputs a function of the level-of-detail inputs and of feature 0 of the level's two grids (each grid one value, different per level) draws, under a straight-down orthographic camera that puts every pixel at one known level of detail, within 3/255 (measured 1/255) of the inline material equal to the CPU reference's blend of mips `m0` and `m0 + 1` at nine levels from -0.5 to 6.4 (magnified, outside the band on both sides, both ends and the middle of the band, across a latent level, past the last mip), in f16 and f32; the same after a 512x512 probe grows the latent texture from one row to eight (the new texture drawn, then the first again); material names that are not ASCII draw without a panic. Putting back each of these defects fails a test: always decoding mip 0, swapped blend weights, a half-level band, the level of detail half a level off, growth without the copy, growth without a new bind group, the byte-sliced suffix check; the constant-network tests alone pass with every one of them |
| `tests/neural_mlp.rs` | `neural_mlp.wgsl` is what its generator writes |

## 8. Decisions and dead ends

- **Training in WGSL, not PyTorch** (charter 4.4 note): there is no PyTorch here, and the encoder
  now runs wherever the renderer does.
- **Bilinear fine grid, not four concatenated taps.** Neural Texture Compression (Vaidyanathan et
  al., 2023) concatenates the fine grid's four taps. With the same bits and a 32-wide network,
  concatenation was 1 to 3 dB worse at mip 0 here and its larger first layer costs more to decode
  (bench 4).
- **8x8-bit latents, not 16x4.** Same bits per texel; 16 four-bit features were within 1 dB either
  way and widen the first layer.
- **A 32-wide network.** 64 wide gains 2 to 4 dB at four times the decode cost; 16 wide loses about
  4 dB.
- **Unrolled by a generated file, not by turning off naga's loop bounds** (3.3).
- **Coarse mips decode worse.** Mip 2 is 5 to 11 dB below mip 0 (the lost branch: 7 to 9), about
  BC1's level: it is the first mip of its level, where the fine grid has a quarter of the mip's
  resolution, each of its texels carries more detail than mip 0's, and it gets an eighth of the
  samples. Sampling coarse mips more often (`--mip-decay 0.7`) moved about 1 dB from mip 0 to mip 2;
  not adopted.
- **One decode call** (3.4), after two slower arrangements.

## 9. Open items

- One network shape per renderer; a second profile would need its own pipelines.
- No alpha channel, so no alpha-masked neural materials.
- Isotropic level of detail only; no anisotropic filtering.
- The training shader still loops (it is offline); unrolling it as the decoder is should speed the
  encoder up severalfold.
- Decoding costs about 0.75 ms per megapixel on the RTX 5060 in f16 and three times that on the
  Radeon 780M (bench 5): fine for a few materials on screen, heavy for a full-screen ground on the
  integrated GPU. Smaller networks, decoding into a cache at lower rate, or the cooperative-matrix
  extensions Neural Texture Compression uses (not in WGSL) are the ways down.
- In Chrome (Dawn on Direct3D 12) the f16 decode costs about what it does natively; f32 up to 1.5
  times (bench 5.3).
