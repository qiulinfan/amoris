//! The block-compressed baseline: each channel group encoded the way a game ships it (Intel's ISPC
//! texture compressor through `intel_tex_2`), decoded back (`texture2ddecoder`) and compared with
//! the same reference as the neural texture.

use intel_tex_2::{RSurface, RgSurface, RgbaSurface, bc1, bc4, bc5, bc7};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Bc1,
    Bc4,
    Bc5,
    Bc7,
}

impl Format {
    pub fn name(self) -> &'static str {
        match self {
            Format::Bc1 => "BC1",
            Format::Bc4 => "BC4",
            Format::Bc5 => "BC5",
            Format::Bc7 => "BC7",
        }
    }
}

/// Encodes up to four channels (`values`, `channels` per texel, in [0, 1]) of a `w`x`h` image
/// and decodes them back; returns the decoded channels (same layout) and the bytes stored.
/// Images smaller than a block or not a multiple of four are padded by repetition.
pub fn round_trip(
    values: &[f32],
    channels: usize,
    w: u32,
    h: u32,
    format: Format,
) -> (Vec<f32>, usize) {
    let pw = w.next_multiple_of(4);
    let ph = h.next_multiple_of(4);
    let byte = |x: u32, y: u32, c: usize| -> u8 {
        if c >= channels {
            return if c == 3 { 255 } else { 0 };
        }
        let v = values[((y % h) * w + (x % w)) as usize * channels + c];
        (v.clamp(0.0, 1.0) * 255.0).round() as u8
    };
    let mut rgba = Vec::with_capacity((pw * ph * 4) as usize);
    for y in 0..ph {
        for x in 0..pw {
            for c in 0..4 {
                rgba.push(byte(x, y, c));
            }
        }
    }
    let surface = RgbaSurface {
        data: &rgba,
        width: pw,
        height: ph,
        stride: pw * 4,
    };
    let blocks = match format {
        Format::Bc1 => bc1::compress_blocks(&surface),
        Format::Bc7 => bc7::compress_blocks(&bc7::opaque_basic_settings(), &surface),
        Format::Bc4 => {
            let r: Vec<u8> = rgba.iter().step_by(4).copied().collect();
            bc4::compress_blocks(&RSurface {
                data: &r,
                width: pw,
                height: ph,
                stride: pw,
            })
        }
        Format::Bc5 => {
            let rg: Vec<u8> = rgba.chunks(4).flat_map(|p| [p[0], p[1]]).collect();
            bc5::compress_blocks(&RgSurface {
                data: &rg,
                width: pw,
                height: ph,
                stride: pw * 2,
            })
        }
    };
    let mut decoded = vec![0u32; (pw * ph) as usize];
    let (pw_, ph_) = (pw as usize, ph as usize);
    match format {
        Format::Bc1 => texture2ddecoder::decode_bc1(&blocks, pw_, ph_, &mut decoded),
        Format::Bc4 => texture2ddecoder::decode_bc4(&blocks, pw_, ph_, &mut decoded),
        Format::Bc5 => texture2ddecoder::decode_bc5(&blocks, pw_, ph_, &mut decoded),
        Format::Bc7 => texture2ddecoder::decode_bc7(&blocks, pw_, ph_, &mut decoded),
    }
    .expect("BCn decode");
    // The decoder writes BGRA words (red in bits 16..24).
    let mut out = Vec::with_capacity((w * h) as usize * channels);
    for y in 0..h {
        for x in 0..w {
            let p = decoded[(y * pw + x) as usize];
            let rgba = [(p >> 16) & 255, (p >> 8) & 255, p & 255, p >> 24];
            for c in rgba.iter().take(channels) {
                out.push(*c as f32 / 255.0);
            }
        }
    }
    let stored = ((w.div_ceil(4) * h.div_ceil(4)) as usize)
        * match format {
            Format::Bc1 | Format::Bc4 => 8,
            Format::Bc5 | Format::Bc7 => 16,
        };
    (out, stored)
}
