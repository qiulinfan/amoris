//! BCn encoding of RGBA8 levels for KTX2 files (Intel's ISPC texture compressor, `intel_tex_2`),
//! shared by the `ktx2` example (the fixture tool) and the renderer's KTX2 tests
//! (docs/spec/texture-formats.md 6).

use amoris_assets::ktx2::{box_chain, level_size};
use amoris_assets::mesh::{BlockFormat, TextureLevels};
use intel_tex_2::{RSurface, RgSurface, RgbaSurface, bc1, bc3, bc4, bc5, bc7};

/// Encodes one `w` x `h` RGBA8 level to `format`. A side that is not a multiple of 4 (a level
/// smaller than a block) is padded by repeating the last row and column; the GPU and the decoder
/// read only the level's own texels.
pub fn encode_level(format: BlockFormat, w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    if format == BlockFormat::Rgba8 {
        return rgba.to_vec();
    }
    let (pw, ph) = (w.next_multiple_of(4), h.next_multiple_of(4));
    let mut padded = Vec::with_capacity((pw * ph * 4) as usize);
    for y in 0..ph {
        for x in 0..pw {
            let at = ((y.min(h - 1) * w + x.min(w - 1)) * 4) as usize;
            padded.extend_from_slice(&rgba[at..at + 4]);
        }
    }
    let surface = RgbaSurface {
        data: &padded,
        width: pw,
        height: ph,
        stride: pw * 4,
    };
    match format {
        BlockFormat::Rgba8 => unreachable!(),
        BlockFormat::Bc1 => bc1::compress_blocks(&surface),
        BlockFormat::Bc3 => bc3::compress_blocks(&surface),
        BlockFormat::Bc7 => bc7::compress_blocks(&bc7::alpha_basic_settings(), &surface),
        BlockFormat::Bc4 => {
            let r: Vec<u8> = padded.iter().step_by(4).copied().collect();
            bc4::compress_blocks(&RSurface {
                data: &r,
                width: pw,
                height: ph,
                stride: pw,
            })
        }
        BlockFormat::Bc5 => {
            let rg: Vec<u8> = padded.chunks(4).flat_map(|p| [p[0], p[1]]).collect();
            bc5::compress_blocks(&RgSurface {
                data: &rg,
                width: pw,
                height: ph,
                stride: pw * 2,
            })
        }
    }
}

/// `rgba` (`w` x `h`) as `format` levels: its full chain of 2 x 2 box averages, or level 0 alone.
pub fn encode(format: BlockFormat, w: u32, h: u32, rgba: &[u8], mips: bool) -> TextureLevels {
    let mut chain = box_chain(w, h, rgba);
    if !mips {
        chain.truncate(1);
    }
    let levels = chain
        .iter()
        .enumerate()
        .map(|(k, level)| {
            let (lw, lh) = level_size(w, h, k as u32);
            encode_level(format, lw, lh, level)
        })
        .collect();
    TextureLevels { format, levels }
}

/// A procedural image that block compression keeps well and Zstd packs small: flat squares of a
/// few colors with a ring across them, `seed` shifting the palette.
pub fn pattern(w: u32, h: u32, seed: u32) -> Vec<u8> {
    let palette = [
        [200u8, 60, 40, 255],
        [40, 140, 200, 255],
        [230, 200, 60, 255],
        [70, 170, 90, 255],
    ];
    let mut px = Vec::with_capacity((w * h * 4) as usize);
    let cell = (w / 8).max(1);
    for y in 0..h {
        for x in 0..w {
            let c = palette[((x / cell + y / cell + seed) % 4) as usize];
            let (dx, dy) = (x as i64 - w as i64 / 2, y as i64 - h as i64 / 2);
            let r2 = dx * dx + dy * dy;
            let ring = w as i64 * 3 / 10;
            let on_ring = (r2 - ring * ring).abs() < ring * (w as i64 / 32).max(1) * 2;
            px.extend_from_slice(&if on_ring { [250, 250, 245, 255] } else { c });
        }
    }
    px
}

/// A tangent-space normal map of the same squares, each tilted its own way.
pub fn normals(w: u32, h: u32) -> Vec<u8> {
    let cell = (w / 8).max(1);
    let tilts = [[160u8, 128], [96, 128], [128, 160], [128, 96]];
    let mut px = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let [nx, ny] = tilts[((x / cell + 2 * (y / cell)) % 4) as usize];
            px.extend_from_slice(&[nx, ny, 240, 255]);
        }
    }
    px
}
