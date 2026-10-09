//! Neural texture compression (charter 4.4; docs/spec/neural-textures.md): a material's channels
//! (base color, normal, occlusion/roughness/metallic, emissive, height) stored as quantized latent
//! grids plus one small MLP that decodes any texel of any mip level, in the fragment shader.
//!
//! The `.ntex` file is little-endian binary, versioned and strictly validated: every size is
//! derived from the header and must match, padding weights must be zero, and no byte may trail.
//! This module is game-side (no GPU, builds for wasm32); the encoder that trains a file and the
//! shader that decodes one live in pocket-render. [`Decoder`] is the CPU reference of the decode
//! every implementation must agree with (the spec's section 3).
//!
//! Decode of mip `m` at `uv` (wrapped to [0, 1)):
//! - level `l = m / 2` holds a fine grid of `(size(2l) >> fine_shift)` latent texels and a coarse
//!   grid of half that (each at least 1x1). A latent texel is 64 bits: `features` unsigned
//!   `bits`-bit values, low bits first, dequantized to `q / (2^bits - 1)`.
//! - the fine grid is read at `p = uv * size - 0.5` with wrapping taps `floor(p)` and `floor(p)+1`:
//!   `Bilinear` blends the four taps, `Taps4` concatenates them (x fastest); the coarse grid is
//!   always blended.
//! - the input vector is fine, coarse, then for octave `k < pe_octaves` and axis x, y the pair
//!   `tri(p * 2^k)`, `tri(p * 2^k + 0.25)` with `tri(t) = |2 fract(t) - 1|` (p in fine texels),
//!   then `m & 1` and `l / 8`, zero-padded to a multiple of four.
//! - two ReLU hidden layers, a linear output; a consumer clamps outputs to [0, 1].

use pocket_sim::math::max_f32;
use serde::{Deserialize, Serialize};

pub const NEURAL_TEXTURE_MAGIC: [u8; 4] = *b"NTEX";
pub const NEURAL_TEXTURE_VERSION: u32 = 1;
/// Largest width or height.
pub const MAX_NEURAL_SIZE: u32 = 4096;
pub const MAX_NEURAL_CHANNELS: usize = 16;
pub const MAX_NEURAL_HIDDEN: u32 = 64;
pub const MAX_NEURAL_NAME: usize = 256;
pub const MAX_NEURAL_FILE_BYTES: usize = 256 * 1024 * 1024;
/// Bits of one latent texel of either grid: two 32-bit words.
pub const LATENT_TEXEL_BITS: u32 = 64;
/// Inputs besides the latents and the positional encoding: `m & 1` and `l / 8`.
pub const LOD_INPUTS: u32 = 2;

/// What a decoded channel means. Codes are stable (the file stores them); channels appear in
/// ascending code order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    /// Base color, sRGB-encoded as in glTF.
    BaseR = 0,
    BaseG = 1,
    BaseB = 2,
    /// Tangent-space normal, encoded `n * 0.5 + 0.5`; z is reconstructed.
    NormalX = 3,
    NormalY = 4,
    /// glTF's occlusion-roughness-metallic, linear.
    Occlusion = 5,
    Roughness = 6,
    Metallic = 7,
    /// Emissive color, sRGB-encoded.
    EmissiveR = 8,
    EmissiveG = 9,
    EmissiveB = 10,
    /// A height (displacement) map, linear; carried, not used by the forward shader.
    Height = 11,
}

impl Channel {
    pub const ALL: [Channel; 12] = [
        Channel::BaseR,
        Channel::BaseG,
        Channel::BaseB,
        Channel::NormalX,
        Channel::NormalY,
        Channel::Occlusion,
        Channel::Roughness,
        Channel::Metallic,
        Channel::EmissiveR,
        Channel::EmissiveG,
        Channel::EmissiveB,
        Channel::Height,
    ];

    pub fn code(self) -> u8 {
        self as u8
    }

    pub fn from_code(code: u8) -> Option<Channel> {
        Channel::ALL.get(code as usize).copied()
    }

    pub fn name(self) -> &'static str {
        match self {
            Channel::BaseR => "base_r",
            Channel::BaseG => "base_g",
            Channel::BaseB => "base_b",
            Channel::NormalX => "normal_x",
            Channel::NormalY => "normal_y",
            Channel::Occlusion => "occlusion",
            Channel::Roughness => "roughness",
            Channel::Metallic => "metallic",
            Channel::EmissiveR => "emissive_r",
            Channel::EmissiveG => "emissive_g",
            Channel::EmissiveB => "emissive_b",
            Channel::Height => "height",
        }
    }

    /// The group a quality report sums the channel into.
    pub fn group(self) -> ChannelGroup {
        match self {
            Channel::BaseR | Channel::BaseG | Channel::BaseB => ChannelGroup::BaseColor,
            Channel::NormalX | Channel::NormalY => ChannelGroup::Normal,
            Channel::Occlusion | Channel::Roughness | Channel::Metallic => ChannelGroup::Orm,
            Channel::EmissiveR | Channel::EmissiveG | Channel::EmissiveB => ChannelGroup::Emissive,
            Channel::Height => ChannelGroup::Height,
        }
    }

    /// Whether the stored value is sRGB-encoded color.
    pub fn srgb(self) -> bool {
        matches!(
            self.group(),
            ChannelGroup::BaseColor | ChannelGroup::Emissive
        )
    }
}

/// Channels that belong together: a material has all of a group's channels or none, except ORM,
/// whose channels are independent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelGroup {
    BaseColor,
    Normal,
    Orm,
    Emissive,
    Height,
}

impl ChannelGroup {
    pub fn name(self) -> &'static str {
        match self {
            ChannelGroup::BaseColor => "base_color",
            ChannelGroup::Normal => "normal",
            ChannelGroup::Orm => "orm",
            ChannelGroup::Emissive => "emissive",
            ChannelGroup::Height => "height",
        }
    }
}

/// How the fine grid's four taps reach the network.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sampling {
    /// Bilinearly blended: `features` inputs.
    Bilinear = 0,
    /// Concatenated, unblended: `4 * features` inputs (the network learns the interpolation).
    Taps4 = 1,
}

/// One latent grid's texel: `features` values of `bits` bits each, 64 bits in all.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GridSpec {
    pub features: u32,
    pub bits: u32,
}

impl GridSpec {
    pub fn validate(&self, what: &str) -> Result<(), String> {
        if !matches!(self.bits, 4 | 8) || self.features * self.bits != LATENT_TEXEL_BITS {
            return Err(format!(
                "{what} grid: {} features of {} bits; a latent texel is 8 features of 8 bits or 16 of 4",
                self.features, self.bits
            ));
        }
        Ok(())
    }

    /// The largest quantized value, `2^bits - 1`.
    pub fn levels(&self) -> u32 {
        (1 << self.bits) - 1
    }

    /// Feature `f` of a texel's two words, dequantized.
    pub fn feature(&self, words: [u32; 2], f: u32) -> f32 {
        let bit = f * self.bits;
        let word = words[(bit / 32) as usize];
        let q = (word >> (bit % 32)) & self.levels();
        q as f32 / self.levels() as f32
    }
}

/// The network and grid shapes of a neural texture.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NeuralLayout {
    pub channels: Vec<Channel>,
    pub fine: GridSpec,
    pub coarse: GridSpec,
    /// The fine grid of level `l` is mip `2l`'s size shifted right by this (1 to 3).
    pub fine_shift: u32,
    pub sampling: Sampling,
    /// Positional-encoding octaves, 0 to 3 (four inputs each).
    pub pe_octaves: u32,
    /// Widths of the two hidden layers, multiples of 4 up to [`MAX_NEURAL_HIDDEN`].
    pub hidden: [u32; 2],
}

fn pad4(n: u32) -> u32 {
    n.div_ceil(4) * 4
}

impl NeuralLayout {
    pub fn validate(&self) -> Result<(), String> {
        if self.channels.is_empty() || self.channels.len() > MAX_NEURAL_CHANNELS {
            return Err(format!(
                "a neural texture has 1 to {MAX_NEURAL_CHANNELS} channels, not {}",
                self.channels.len()
            ));
        }
        if self.channels.windows(2).any(|w| w[0] >= w[1]) {
            return Err(
                "neural texture channels must be distinct and in ascending code order".into(),
            );
        }
        for (group, members) in [
            (
                ChannelGroup::BaseColor,
                &[Channel::BaseR, Channel::BaseG, Channel::BaseB][..],
            ),
            (ChannelGroup::Normal, &[Channel::NormalX, Channel::NormalY]),
            (
                ChannelGroup::Emissive,
                &[Channel::EmissiveR, Channel::EmissiveG, Channel::EmissiveB],
            ),
        ] {
            let present = members.iter().filter(|c| self.channels.contains(c)).count();
            if present != 0 && present != members.len() {
                return Err(format!(
                    "neural texture group {} needs all of its channels or none",
                    group.name()
                ));
            }
        }
        self.fine.validate("fine")?;
        self.coarse.validate("coarse")?;
        if !(1..=3).contains(&self.fine_shift) {
            return Err(format!(
                "fine_shift must be 1 to 3, not {}",
                self.fine_shift
            ));
        }
        if self.pe_octaves > 3 {
            return Err(format!(
                "pe_octaves must be 0 to 3, not {}",
                self.pe_octaves
            ));
        }
        for h in self.hidden {
            if h == 0 || h % 4 != 0 || h > MAX_NEURAL_HIDDEN {
                return Err(format!(
                    "hidden widths must be multiples of 4 up to {MAX_NEURAL_HIDDEN}, not {h}"
                ));
            }
        }
        Ok(())
    }

    pub fn fine_inputs(&self) -> u32 {
        match self.sampling {
            Sampling::Bilinear => self.fine.features,
            Sampling::Taps4 => 4 * self.fine.features,
        }
    }

    /// The input vector's length before padding.
    pub fn inputs(&self) -> u32 {
        self.fine_inputs() + self.coarse.features + 4 * self.pe_octaves + LOD_INPUTS
    }

    /// The input vector's length, padded to a multiple of four.
    pub fn inputs_padded(&self) -> u32 {
        pad4(self.inputs())
    }

    pub fn outputs_padded(&self) -> u32 {
        pad4(self.channels.len() as u32)
    }

    /// The layers as (outputs, inputs), padded.
    pub fn layers(&self) -> [(u32, u32); 3] {
        [
            (self.hidden[0], self.inputs_padded()),
            (self.hidden[1], self.hidden[0]),
            (self.outputs_padded(), self.hidden[1]),
        ]
    }

    /// Where each layer's weights (row-major, outputs by inputs) and biases start.
    pub fn offsets(&self) -> [(usize, usize); 3] {
        let mut at = 0usize;
        self.layers().map(|(o, i)| {
            let w = at;
            let b = w + (o * i) as usize;
            at = b + o as usize;
            (w, b)
        })
    }

    pub fn weight_count(&self) -> usize {
        self.layers()
            .iter()
            .map(|&(o, i)| (o * i + o) as usize)
            .sum()
    }

    /// Whether weight `index` is padding (an input column past [`inputs`](Self::inputs), or an
    /// output row past the channels), which must stay zero.
    pub fn is_padding(&self, index: usize) -> bool {
        let [(w1, b1), _, (w3, b3)] = self.offsets();
        let inputs = self.inputs_padded() as usize;
        let channels = self.channels.len();
        let hidden = self.hidden[1] as usize;
        if index >= w1 && index < b1 {
            return (index - w1) % inputs >= self.inputs() as usize;
        }
        if index >= w3 && index < b3 {
            return (index - w3) / hidden >= channels;
        }
        if index >= b3 {
            return index - b3 >= channels;
        }
        false
    }

    /// The index of a channel in the output, if present.
    pub fn output_of(&self, channel: Channel) -> Option<usize> {
        self.channels.iter().position(|&c| c == channel)
    }
}

/// The latent grid sizes of one level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LevelDims {
    pub fine: [u32; 2],
    pub coarse: [u32; 2],
}

/// Mip `m`'s size.
pub fn mip_size(width: u32, height: u32, m: u32) -> [u32; 2] {
    [(width >> m).max(1), (height >> m).max(1)]
}

/// The full mip chain's length for a texture.
pub fn full_mip_count(width: u32, height: u32) -> u32 {
    width.max(height).max(1).ilog2() + 1
}

/// The latent levels a texture of this size and mip count has (one per two mips).
pub fn level_dims(layout: &NeuralLayout, width: u32, height: u32, mips: u32) -> Vec<LevelDims> {
    (0..mips.div_ceil(2))
        .map(|l| {
            let [w, h] = mip_size(width, height, 2 * l);
            let fine = [
                (w >> layout.fine_shift).max(1),
                (h >> layout.fine_shift).max(1),
            ];
            LevelDims {
                fine,
                coarse: [(fine[0] >> 1).max(1), (fine[1] >> 1).max(1)],
            }
        })
        .collect()
}

/// One latent grid: two words per texel, row-major.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LatentGrid {
    pub width: u32,
    pub height: u32,
    pub words: Vec<u32>,
}

impl LatentGrid {
    pub fn texel(&self, x: u32, y: u32) -> [u32; 2] {
        let i = 2 * (y * self.width + x) as usize;
        [self.words[i], self.words[i + 1]]
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LatentLevel {
    pub fine: LatentGrid,
    pub coarse: LatentGrid,
}

/// A trained neural texture.
#[derive(Clone, Debug, PartialEq)]
pub struct NeuralTexture {
    /// A label (the material's name), at most [`MAX_NEURAL_NAME`] bytes of UTF-8.
    pub name: String,
    /// Width and height of mip 0: powers of two up to [`MAX_NEURAL_SIZE`].
    pub width: u32,
    pub height: u32,
    /// Mips the file decodes, 1 to the full chain.
    pub mip_count: u32,
    pub layout: NeuralLayout,
    pub levels: Vec<LatentLevel>,
    /// The network as IEEE half floats, layer by layer: weights (row-major, outputs by padded
    /// inputs), then biases.
    pub weights: Vec<u16>,
}

/// Sizes of a texture's parts, for reports.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct NeuralSize {
    pub latent_bytes: usize,
    pub weight_bytes: usize,
    pub file_bytes: usize,
    /// Latents and weights per texel of mip 0, every mip included.
    pub bits_per_texel: f64,
}

impl NeuralTexture {
    pub fn validate(&self) -> Result<(), String> {
        self.layout.validate()?;
        if self.name.len() > MAX_NEURAL_NAME {
            return Err(format!(
                "neural texture name is longer than {MAX_NEURAL_NAME} bytes"
            ));
        }
        for (what, v) in [("width", self.width), ("height", self.height)] {
            if !v.is_power_of_two() || v > MAX_NEURAL_SIZE {
                return Err(format!(
                    "neural texture {what} {v} is not a power of two up to {MAX_NEURAL_SIZE}"
                ));
            }
        }
        let full = full_mip_count(self.width, self.height);
        if self.mip_count == 0 || self.mip_count > full {
            return Err(format!(
                "neural texture has {} mips; a {}x{} texture has 1 to {full}",
                self.mip_count, self.width, self.height
            ));
        }
        let dims = level_dims(&self.layout, self.width, self.height, self.mip_count);
        if dims.len() != self.levels.len() {
            return Err(format!(
                "neural texture has {} latent levels; {} mips need {}",
                self.levels.len(),
                self.mip_count,
                dims.len()
            ));
        }
        for (l, (level, d)) in self.levels.iter().zip(&dims).enumerate() {
            for (what, grid, want) in [
                ("fine", &level.fine, d.fine),
                ("coarse", &level.coarse, d.coarse),
            ] {
                if [grid.width, grid.height] != want {
                    return Err(format!(
                        "level {l} {what} grid is {}x{}, the layout needs {}x{}",
                        grid.width, grid.height, want[0], want[1]
                    ));
                }
                if grid.words.len() != 2 * (want[0] * want[1]) as usize {
                    return Err(format!(
                        "level {l} {what} grid has the wrong number of words"
                    ));
                }
            }
        }
        if self.weights.len() != self.layout.weight_count() {
            return Err(format!(
                "neural texture has {} weights; the layout needs {}",
                self.weights.len(),
                self.layout.weight_count()
            ));
        }
        for (i, &w) in self.weights.iter().enumerate() {
            if !f16_to_f32(w).is_finite() {
                return Err(format!("weight {i} is not finite"));
            }
            if self.layout.is_padding(i) && f16_to_f32(w) != 0.0 {
                return Err(format!("weight {i} is padding and must be zero"));
            }
        }
        Ok(())
    }

    pub fn size(&self) -> NeuralSize {
        let latent_bytes: usize = self
            .levels
            .iter()
            .map(|l| 4 * (l.fine.words.len() + l.coarse.words.len()))
            .sum();
        let weight_bytes = 2 * self.weights.len();
        let file_bytes = self.to_bytes().len();
        NeuralSize {
            latent_bytes,
            weight_bytes,
            file_bytes,
            bits_per_texel: 8.0 * (latent_bytes + weight_bytes) as f64
                / (self.width as f64 * self.height as f64),
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let u32s = |out: &mut Vec<u8>, vs: &[u32]| {
            for v in vs {
                out.extend_from_slice(&v.to_le_bytes());
            }
        };
        out.extend_from_slice(&NEURAL_TEXTURE_MAGIC);
        let l = &self.layout;
        u32s(
            &mut out,
            &[
                NEURAL_TEXTURE_VERSION,
                self.width,
                self.height,
                self.mip_count,
                l.channels.len() as u32,
            ],
        );
        let mut codes = [0xffu8; MAX_NEURAL_CHANNELS];
        for (slot, c) in codes.iter_mut().zip(&l.channels) {
            *slot = c.code();
        }
        out.extend_from_slice(&codes);
        u32s(
            &mut out,
            &[
                l.fine.features,
                l.fine.bits,
                l.coarse.features,
                l.coarse.bits,
                l.fine_shift,
                l.sampling as u32,
                l.pe_octaves,
                l.hidden[0],
                l.hidden[1],
                self.name.len() as u32,
            ],
        );
        out.extend_from_slice(self.name.as_bytes());
        u32s(&mut out, &[self.levels.len() as u32]);
        for level in &self.levels {
            u32s(
                &mut out,
                &[
                    level.fine.width,
                    level.fine.height,
                    level.coarse.width,
                    level.coarse.height,
                ],
            );
            u32s(&mut out, &level.fine.words);
            u32s(&mut out, &level.coarse.words);
        }
        u32s(&mut out, &[self.weights.len() as u32]);
        for w in &self.weights {
            out.extend_from_slice(&w.to_le_bytes());
        }
        if self.weights.len() % 2 == 1 {
            out.extend_from_slice(&[0, 0]);
        }
        out
    }

    /// Strictly decodes and validates a `.ntex` file.
    pub fn from_bytes(bytes: &[u8]) -> Result<NeuralTexture, String> {
        if bytes.len() > MAX_NEURAL_FILE_BYTES {
            return Err(format!(
                "neural texture is {} bytes; maximum is {MAX_NEURAL_FILE_BYTES}",
                bytes.len()
            ));
        }
        let mut r = Reader { bytes, at: 0 };
        if r.take(4)? != NEURAL_TEXTURE_MAGIC {
            return Err("not a neural texture (no NTEX magic)".into());
        }
        let version = r.u32()?;
        if version != NEURAL_TEXTURE_VERSION {
            return Err(format!(
                "neural texture version {version}; this build reads {NEURAL_TEXTURE_VERSION}"
            ));
        }
        let width = r.u32()?;
        let height = r.u32()?;
        let mip_count = r.u32()?;
        let channel_count = r.u32()? as usize;
        if channel_count == 0 || channel_count > MAX_NEURAL_CHANNELS {
            return Err(format!("neural texture has {channel_count} channels"));
        }
        let codes = r.take(MAX_NEURAL_CHANNELS)?;
        let mut channels = Vec::with_capacity(channel_count);
        for (i, &code) in codes.iter().enumerate() {
            if i < channel_count {
                channels.push(
                    Channel::from_code(code)
                        .ok_or_else(|| format!("unknown neural texture channel code {code}"))?,
                );
            } else if code != 0xff {
                return Err("unused channel slots must be 0xff".into());
            }
        }
        let fine = GridSpec {
            features: r.u32()?,
            bits: r.u32()?,
        };
        let coarse = GridSpec {
            features: r.u32()?,
            bits: r.u32()?,
        };
        let fine_shift = r.u32()?;
        let sampling = match r.u32()? {
            0 => Sampling::Bilinear,
            1 => Sampling::Taps4,
            s => return Err(format!("unknown neural texture sampling {s}")),
        };
        let pe_octaves = r.u32()?;
        let hidden = [r.u32()?, r.u32()?];
        let layout = NeuralLayout {
            channels,
            fine,
            coarse,
            fine_shift,
            sampling,
            pe_octaves,
            hidden,
        };
        layout.validate()?;
        let name_len = r.u32()? as usize;
        if name_len > MAX_NEURAL_NAME {
            return Err("neural texture name is too long".into());
        }
        let name = std::str::from_utf8(r.take(name_len)?)
            .map_err(|_| "neural texture name is not UTF-8".to_string())?
            .to_owned();
        for (what, v) in [("width", width), ("height", height)] {
            if !v.is_power_of_two() || v > MAX_NEURAL_SIZE {
                return Err(format!(
                    "neural texture {what} {v} is not a power of two up to {MAX_NEURAL_SIZE}"
                ));
            }
        }
        if mip_count == 0 || mip_count > full_mip_count(width, height) {
            return Err(format!(
                "neural texture mip count {mip_count} is out of range"
            ));
        }
        let dims = level_dims(&layout, width, height, mip_count);
        let level_count = r.u32()? as usize;
        if level_count != dims.len() {
            return Err(format!(
                "neural texture has {level_count} latent levels; {mip_count} mips need {}",
                dims.len()
            ));
        }
        let mut levels = Vec::with_capacity(level_count);
        for (l, d) in dims.iter().enumerate() {
            let sizes = [r.u32()?, r.u32()?, r.u32()?, r.u32()?];
            if sizes != [d.fine[0], d.fine[1], d.coarse[0], d.coarse[1]] {
                return Err(format!(
                    "level {l} grids are {sizes:?}; the layout needs {:?} and {:?}",
                    d.fine, d.coarse
                ));
            }
            let fine = LatentGrid {
                width: d.fine[0],
                height: d.fine[1],
                words: r.u32s(2 * (d.fine[0] * d.fine[1]) as usize)?,
            };
            let coarse = LatentGrid {
                width: d.coarse[0],
                height: d.coarse[1],
                words: r.u32s(2 * (d.coarse[0] * d.coarse[1]) as usize)?,
            };
            levels.push(LatentLevel { fine, coarse });
        }
        let weight_count = r.u32()? as usize;
        if weight_count != layout.weight_count() {
            return Err(format!(
                "neural texture has {weight_count} weights; the layout needs {}",
                layout.weight_count()
            ));
        }
        let raw = r.take(2 * weight_count)?;
        let weights: Vec<u16> = raw
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&b| u16::from_le_bytes(b))
            .collect();
        if weight_count % 2 == 1 && r.take(2)? != [0, 0] {
            return Err("neural texture weight padding must be zero".into());
        }
        if r.at != bytes.len() {
            return Err(format!(
                "neural texture has {} trailing bytes",
                bytes.len() - r.at
            ));
        }
        let texture = NeuralTexture {
            name,
            width,
            height,
            mip_count,
            layout,
            levels,
            weights,
        };
        texture.validate()?;
        Ok(texture)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn load(path: &std::path::Path) -> Result<NeuralTexture, String> {
        use std::io::Read;
        let file = std::fs::File::open(path)
            .map_err(|e| format!("cannot open neural texture {}: {e}", path.display()))?;
        let mut bytes = Vec::new();
        file.take(MAX_NEURAL_FILE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| format!("cannot read neural texture {}: {e}", path.display()))?;
        NeuralTexture::from_bytes(&bytes).map_err(|e| format!("{}: {e}", path.display()))
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self
            .at
            .checked_add(n)
            .filter(|&e| e <= self.bytes.len())
            .ok_or_else(|| "neural texture is truncated".to_string())?;
        let s = &self.bytes[self.at..end];
        self.at = end;
        Ok(s)
    }

    fn u32(&mut self) -> Result<u32, String> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn u32s(&mut self, n: usize) -> Result<Vec<u32>, String> {
        let bytes = self.take(n.checked_mul(4).ok_or("neural texture size overflows")?)?;
        Ok(bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&b| u32::from_le_bytes(b))
            .collect())
    }
}

/// IEEE 754 half to single precision (exact).
pub fn f16_to_f32(h: u16) -> f32 {
    let sign = u32::from(h >> 15) << 31;
    let exponent = u32::from((h >> 10) & 0x1f);
    let mantissa = u32::from(h & 0x3ff);
    let bits = match (exponent, mantissa) {
        (0, 0) => sign,
        (0, m) => {
            // Subnormal: normalize.
            let shift = m.leading_zeros() - 21;
            let m = (m << shift) & 0x3ff;
            sign | ((113 - shift) << 23) | (m << 13)
        }
        (0x1f, 0) => sign | 0x7f80_0000,
        (0x1f, m) => sign | 0x7fc0_0000 | (m << 13),
        (e, m) => sign | ((e + 112) << 23) | (m << 13),
    };
    f32::from_bits(bits)
}

/// Single to half precision, rounding to nearest even; out-of-range values become infinities.
pub fn f32_to_f16(f: f32) -> u16 {
    let bits = f.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exponent = ((bits >> 23) & 0xff) as i32;
    let mantissa = bits & 0x7f_ffff;
    if exponent == 0xff {
        return sign | 0x7c00 | if mantissa != 0 { 0x200 } else { 0 };
    }
    let e = exponent - 127 + 15;
    if e >= 0x1f {
        return sign | 0x7c00;
    }
    if e <= 0 {
        if e < -10 {
            return sign;
        }
        // Subnormal half: shift the full mantissa (with its implicit one) into place.
        let m = mantissa | 0x80_0000;
        let shift = (14 - e) as u32;
        let half = m >> shift;
        let rest = m & ((1 << shift) - 1);
        let halfway = 1 << (shift - 1);
        let round = rest > halfway || (rest == halfway && half & 1 == 1);
        return sign | (half + u32::from(round)) as u16;
    }
    let half = ((e as u32) << 10) | (mantissa >> 13);
    let rest = mantissa & 0x1fff;
    let round = rest > 0x1000 || (rest == 0x1000 && half & 1 == 1);
    // A carry out of the mantissa correctly bumps the exponent (up to infinity).
    sign | (half + u32::from(round)) as u16
}

/// `|2 fract(t) - 1|`: 1 at integers, 0 halfway.
pub fn tri(t: f32) -> f32 {
    (2.0 * (t - t.floor()) - 1.0).abs()
}

/// The CPU reference decoder (f32 arithmetic on the file's f16 weights).
pub struct Decoder<'a> {
    pub texture: &'a NeuralTexture,
    weights: Vec<f32>,
}

impl<'a> Decoder<'a> {
    pub fn new(texture: &'a NeuralTexture) -> Decoder<'a> {
        Decoder {
            texture,
            weights: texture.weights.iter().map(|&w| f16_to_f32(w)).collect(),
        }
    }

    /// The network's input for mip `mip` at `uv` (padded length).
    pub fn inputs(&self, mip: u32, uv: [f32; 2]) -> Vec<f32> {
        let t = self.texture;
        let layout = &t.layout;
        let level = &t.levels[(mip / 2) as usize];
        let mut x = Vec::with_capacity(layout.inputs_padded() as usize);
        let u = [uv[0] - uv[0].floor(), uv[1] - uv[1].floor()];
        let taps = |grid: &LatentGrid| {
            let size = [grid.width, grid.height];
            let p: [f32; 2] = std::array::from_fn(|a| u[a] * size[a] as f32 - 0.5);
            let i0: [i64; 2] = std::array::from_fn(|a| p[a].floor() as i64);
            let f: [f32; 2] = std::array::from_fn(|a| p[a] - i0[a] as f32);
            let wrap = |i: i64, n: u32| i.rem_euclid(i64::from(n)) as u32;
            let tap =
                |dx: i64, dy: i64| grid.texel(wrap(i0[0] + dx, size[0]), wrap(i0[1] + dy, size[1]));
            let words = [tap(0, 0), tap(1, 0), tap(0, 1), tap(1, 1)];
            let weights = [
                (1.0 - f[0]) * (1.0 - f[1]),
                f[0] * (1.0 - f[1]),
                (1.0 - f[0]) * f[1],
                f[0] * f[1],
            ];
            (words, weights, p)
        };
        let (fw, fweights, p) = taps(&level.fine);
        match layout.sampling {
            Sampling::Bilinear => {
                for f in 0..layout.fine.features {
                    x.push(
                        (0..4)
                            .map(|k| fweights[k] * layout.fine.feature(fw[k], f))
                            .sum(),
                    );
                }
            }
            Sampling::Taps4 => {
                for words in fw {
                    for f in 0..layout.fine.features {
                        x.push(layout.fine.feature(words, f));
                    }
                }
            }
        }
        let (cw, cweights, _) = taps(&level.coarse);
        for f in 0..layout.coarse.features {
            x.push(
                (0..4)
                    .map(|k| cweights[k] * layout.coarse.feature(cw[k], f))
                    .sum(),
            );
        }
        for k in 0..layout.pe_octaves {
            let scale = (1u32 << k) as f32;
            for axis in p {
                x.push(tri(axis * scale));
                x.push(tri(axis * scale + 0.25));
            }
        }
        x.push((mip & 1) as f32);
        x.push((mip / 2) as f32 / 8.0);
        x.resize(layout.inputs_padded() as usize, 0.0);
        x
    }

    /// The decoded channels (unclamped) of mip `mip` at `uv`.
    pub fn decode(&self, mip: u32, uv: [f32; 2]) -> Vec<f32> {
        let layout = &self.texture.layout;
        let mut x = self.inputs(mip, uv);
        let offsets = layout.offsets();
        for (layer, &(outputs, inputs)) in layout.layers().iter().enumerate() {
            let (w, b) = offsets[layer];
            let y: Vec<f32> = (0..outputs as usize)
                .map(|o| {
                    let mut v = self.weights[b + o];
                    for (i, xi) in x.iter().enumerate() {
                        v += self.weights[w + o * inputs as usize + i] * xi;
                    }
                    if layer < 2 { max_f32(v, 0.0) } else { v }
                })
                .collect();
            x = y;
        }
        x.truncate(layout.channels.len());
        x
    }

    /// Texel `(x, y)` of mip `mip`, decoded at its centre.
    pub fn decode_texel(&self, mip: u32, x: u32, y: u32) -> Vec<f32> {
        let [w, h] = mip_size(self.texture.width, self.texture.height, mip);
        self.decode(
            mip,
            [(x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> NeuralLayout {
        NeuralLayout {
            channels: vec![
                Channel::BaseR,
                Channel::BaseG,
                Channel::BaseB,
                Channel::NormalX,
                Channel::NormalY,
                Channel::Roughness,
            ],
            fine: GridSpec {
                features: 8,
                bits: 8,
            },
            coarse: GridSpec {
                features: 16,
                bits: 4,
            },
            fine_shift: 2,
            sampling: Sampling::Bilinear,
            pe_octaves: 2,
            hidden: [8, 4],
        }
    }

    /// A small texture with patterned latents and weights.
    fn texture() -> NeuralTexture {
        let layout = layout();
        let (width, height, mips) = (16, 8, full_mip_count(16, 8));
        let mut seed = 7u32;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        let levels = level_dims(&layout, width, height, mips)
            .iter()
            .map(|d| {
                let mut grid = |[w, h]: [u32; 2]| LatentGrid {
                    width: w,
                    height: h,
                    words: (0..2 * w * h).map(|_| next()).collect(),
                };
                LatentLevel {
                    fine: grid(d.fine),
                    coarse: grid(d.coarse),
                }
            })
            .collect();
        let weights = (0..layout.weight_count())
            .map(|i| {
                if layout.is_padding(i) {
                    0
                } else {
                    f32_to_f16((next() % 2001) as f32 / 1000.0 - 1.0)
                }
            })
            .collect();
        NeuralTexture {
            name: "test".into(),
            width,
            height,
            mip_count: mips,
            layout,
            levels,
            weights,
        }
    }

    #[test]
    fn half_conversion_round_trips_every_finite_half() {
        for h in 0..=u16::MAX {
            let f = f16_to_f32(h);
            if f.is_nan() {
                assert!(f32_to_f16(f) & 0x7c00 == 0x7c00 && f32_to_f16(f) & 0x3ff != 0);
                continue;
            }
            assert_eq!(f32_to_f16(f), h, "half {h:#06x} -> {f}");
        }
        assert_eq!(f16_to_f32(0x3c00), 1.0);
        assert_eq!(f16_to_f32(0xc000), -2.0);
        assert_eq!(f16_to_f32(0x0001), f32::from_bits(0x3380_0000));
        assert_eq!(f32_to_f16(65520.0), 0x7c00);
        assert_eq!(f32_to_f16(1.0 + 1.0 / 2048.0), 0x3c00, "ties go to even");
        assert_eq!(f32_to_f16(1.0 + 3.0 / 2048.0), 0x3c02, "ties go to even");
        assert_eq!(f32_to_f16(1e-9), 0);
    }

    #[test]
    fn layout_sizes() {
        let l = layout();
        assert_eq!(l.inputs(), 8 + 16 + 8 + 2);
        assert_eq!(l.inputs_padded(), 36);
        assert_eq!(l.outputs_padded(), 8);
        assert_eq!(l.weight_count(), 8 * 36 + 8 + 4 * 8 + 4 + 8 * 4 + 8);
        let padding: Vec<usize> = (0..l.weight_count()).filter(|&i| l.is_padding(i)).collect();
        // Two padded input columns per first-layer row, two padded output rows and their biases.
        assert_eq!(padding.len(), 2 * 8 + 2 * 4 + 2);
        let dims = level_dims(&l, 16, 8, full_mip_count(16, 8));
        assert_eq!(dims.len(), 3);
        assert_eq!(dims[0].fine, [4, 2]);
        assert_eq!(dims[0].coarse, [2, 1]);
        assert_eq!(dims[2].fine, [1, 1]);
    }

    #[test]
    fn bytes_round_trip() {
        let t = texture();
        t.validate().unwrap();
        let bytes = t.to_bytes();
        assert_eq!(NeuralTexture::from_bytes(&bytes).unwrap(), t);
        assert_eq!(t.size().file_bytes, bytes.len());
    }

    #[test]
    fn malformed_files_are_rejected() {
        let t = texture();
        let bytes = t.to_bytes();
        let reject = |b: &[u8], what: &str| {
            let e = NeuralTexture::from_bytes(b).expect_err(what);
            assert!(!e.is_empty());
        };
        reject(&bytes[..bytes.len() - 1], "truncated");
        let mut long = bytes.clone();
        long.push(0);
        reject(&long, "trailing byte");
        let mut magic = bytes.clone();
        magic[0] = b'X';
        reject(&magic, "magic");
        let mut version = bytes.clone();
        version[4] = 2;
        reject(&version, "version");
        let mut width = bytes.clone();
        width[8] = 12;
        reject(&width, "width not a power of two");
        let mut channel = bytes.clone();
        channel[24 + 1] = Channel::BaseR.code();
        reject(&channel, "duplicate channel");
        let mut unused = bytes.clone();
        unused[24 + 15] = 3;
        reject(&unused, "unused channel slot");
        let mut bits = bytes.clone();
        bits[44] = 6;
        reject(&bits, "latent bits");

        let mut partial = t.clone();
        partial.layout.channels.retain(|&c| c != Channel::BaseG);
        assert!(partial.validate().is_err(), "partial base color");
        let mut padding = t.clone();
        let i = (0..t.weights.len())
            .find(|&i| t.layout.is_padding(i))
            .unwrap();
        padding.weights[i] = f32_to_f16(0.5);
        assert!(NeuralTexture::from_bytes(&padding.to_bytes()).is_err());
        let mut nan = t.clone();
        nan.weights[3] = 0x7e00;
        assert!(NeuralTexture::from_bytes(&nan.to_bytes()).is_err());
        let mut grid = t.clone();
        grid.levels[0].fine.words.pop();
        grid.levels[0].fine.words.pop();
        assert!(grid.validate().is_err());
        let mut mips = t.clone();
        mips.mip_count = 9;
        assert!(mips.validate().is_err());
    }

    #[test]
    fn features_unpack_low_bits_first() {
        let eight = GridSpec {
            features: 8,
            bits: 8,
        };
        let words = [0x0403_02ff, 0x8000_0000];
        assert_eq!(eight.feature(words, 0), 1.0);
        assert_eq!(eight.feature(words, 1), 2.0 / 255.0);
        assert_eq!(eight.feature(words, 3), 4.0 / 255.0);
        assert_eq!(eight.feature(words, 7), 128.0 / 255.0);
        let four = GridSpec {
            features: 16,
            bits: 4,
        };
        assert_eq!(four.feature(words, 0), 1.0);
        assert_eq!(four.feature(words, 1), 1.0);
        assert_eq!(four.feature(words, 2), 2.0 / 15.0);
        assert_eq!(four.feature(words, 15), 8.0 / 15.0);
    }

    #[test]
    fn inputs_follow_the_spec() {
        let t = texture();
        let d = Decoder::new(&t);
        // At a fine latent texel's centre the blend is that texel exactly; the grid wraps.
        let fine = &t.levels[0].fine;
        let uv = [0.5 / fine.width as f32, 0.5 / fine.height as f32];
        let x = d.inputs(0, uv);
        for f in 0..8 {
            assert!((x[f as usize] - t.layout.fine.feature(fine.texel(0, 0), f)).abs() < 1e-6);
        }
        // Halfway between texel 0 and the last texel (wrapping) of a row.
        let x = d.inputs(0, [0.0, uv[1]]);
        let expect = 0.5 * t.layout.fine.feature(fine.texel(0, 0), 0)
            + 0.5 * t.layout.fine.feature(fine.texel(fine.width - 1, 0), 0);
        assert!((x[0] - expect).abs() < 1e-6);
        // Positional encoding at a latent centre: tri(integer) = 1, tri(+0.25) = 0.5.
        let x = d.inputs(0, uv);
        assert_eq!(&x[24..28], &[1.0, 0.5, 1.0, 0.5]);
        // Lod inputs, then zero padding.
        let x = d.inputs(3, uv);
        assert_eq!(&x[32..36], &[1.0, 1.0 / 8.0, 0.0, 0.0]);
        // Taps4 concatenates the four taps, x fastest.
        let mut t4 = t.clone();
        t4.layout.sampling = Sampling::Taps4;
        let d4 = Decoder::new(&t4);
        let x = d4.inputs(0, [1.0 / fine.width as f32, uv[1]]);
        assert_eq!(x[0], t4.layout.fine.feature(fine.texel(0, 0), 0));
        assert_eq!(x[8], t4.layout.fine.feature(fine.texel(1, 0), 0));
        assert_eq!(x[16], t4.layout.fine.feature(fine.texel(0, 1), 0));
    }

    #[test]
    fn decoder_matches_a_hand_evaluation() {
        let t = texture();
        let d = Decoder::new(&t);
        let uv = [0.37, 0.81];
        let x = d.inputs(1, uv);
        let w: Vec<f32> = t.weights.iter().map(|&h| f16_to_f32(h)).collect();
        let [(w1, b1), (w2, b2), (w3, b3)] = t.layout.offsets();
        let h1: Vec<f32> = (0..8)
            .map(|o| {
                max_f32(
                    w[b1 + o] + (0..36).map(|i| w[w1 + o * 36 + i] * x[i]).sum::<f32>(),
                    0.0,
                )
            })
            .collect();
        let h2: Vec<f32> = (0..4)
            .map(|o| {
                max_f32(
                    w[b2 + o] + (0..8).map(|i| w[w2 + o * 8 + i] * h1[i]).sum::<f32>(),
                    0.0,
                )
            })
            .collect();
        let y: Vec<f32> = (0..6)
            .map(|o| w[b3 + o] + (0..4).map(|i| w[w3 + o * 4 + i] * h2[i]).sum::<f32>())
            .collect();
        let got = d.decode(1, uv);
        for (a, b) in got.iter().zip(&y) {
            assert!((a - b).abs() < 1e-5, "{got:?} != {y:?}");
        }
    }
}
