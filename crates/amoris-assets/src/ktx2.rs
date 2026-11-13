//! KTX2 textures (docs/spec/texture-formats.md): reading a `.ktx2` file's levels, undoing its Zstd
//! supercompression, decoding block-compressed levels to RGBA8 on the CPU (where the GPU samples
//! no block format, and for the CPU's own readers), and writing files (the fixture tool and tests).
//! Pure Rust (`ktx2`, `ruzstd`, `texture2ddecoder`): it builds for `wasm32` like the importer.

use std::io::Read;

use crate::mesh::{BlockFormat, ImageData, TextureLevels};

/// The largest side read: a 16384 x 16384 BC7 level is 256 MiB, RGBA8 1 GiB.
pub const MAX_SIDE: u32 = 16384;

/// Whether `bytes` start with KTX 2's identifier.
pub fn is_ktx2(bytes: &[u8]) -> bool {
    bytes.starts_with(&[
        0xAB, b'K', b'T', b'X', b' ', b'2', b'0', 0xBB, b'\r', b'\n', 0x1A, b'\n',
    ])
}

/// A KTX2 file's image: its size, whether the file calls its values sRGB-encoded (glTF's use of
/// the image decides how the renderer reads them, section 2), and its levels.
#[derive(Clone, Debug, PartialEq)]
pub struct Ktx2Image {
    pub width: u32,
    pub height: u32,
    pub srgb: bool,
    pub levels: TextureLevels,
}

/// The number of levels of a full chain down to 1 x 1 for a `width` x `height` image.
pub fn full_chain(width: u32, height: u32) -> u32 {
    width.max(height).max(1).ilog2() + 1
}

/// The size of level `k` of a `width` x `height` image.
pub fn level_size(width: u32, height: u32, k: u32) -> (u32, u32) {
    ((width >> k).max(1), (height >> k).max(1))
}

fn format_of(vk: u32) -> Option<(BlockFormat, bool)> {
    use ktx2::Format as F;
    let f = F::new(vk)?;
    Some(match f {
        F::R8G8B8A8_UNORM => (BlockFormat::Rgba8, false),
        F::R8G8B8A8_SRGB => (BlockFormat::Rgba8, true),
        // WebGPU's only BC1 is bc1-rgba: the 3-color mode's fourth color is transparent black.
        F::BC1_RGB_UNORM_BLOCK | F::BC1_RGBA_UNORM_BLOCK => (BlockFormat::Bc1, false),
        F::BC1_RGB_SRGB_BLOCK | F::BC1_RGBA_SRGB_BLOCK => (BlockFormat::Bc1, true),
        F::BC3_UNORM_BLOCK => (BlockFormat::Bc3, false),
        F::BC3_SRGB_BLOCK => (BlockFormat::Bc3, true),
        F::BC4_UNORM_BLOCK => (BlockFormat::Bc4, false),
        F::BC5_UNORM_BLOCK => (BlockFormat::Bc5, false),
        F::BC7_UNORM_BLOCK => (BlockFormat::Bc7, false),
        F::BC7_SRGB_BLOCK => (BlockFormat::Bc7, true),
        _ => return None,
    })
}

fn vk_format(format: BlockFormat, srgb: bool) -> ktx2::Format {
    use ktx2::Format as F;
    match (format, srgb) {
        (BlockFormat::Rgba8, false) => F::R8G8B8A8_UNORM,
        (BlockFormat::Rgba8, true) => F::R8G8B8A8_SRGB,
        (BlockFormat::Bc1, false) => F::BC1_RGBA_UNORM_BLOCK,
        (BlockFormat::Bc1, true) => F::BC1_RGBA_SRGB_BLOCK,
        (BlockFormat::Bc3, false) => F::BC3_UNORM_BLOCK,
        (BlockFormat::Bc3, true) => F::BC3_SRGB_BLOCK,
        (BlockFormat::Bc4, _) => F::BC4_UNORM_BLOCK,
        (BlockFormat::Bc5, _) => F::BC5_UNORM_BLOCK,
        (BlockFormat::Bc7, false) => F::BC7_UNORM_BLOCK,
        (BlockFormat::Bc7, true) => F::BC7_SRGB_BLOCK,
    }
}

/// Reads a KTX2 file: one 2D image (no array layers, cube faces or depth), RGBA8 or BC1, BC3, BC4,
/// BC5 or BC7, stored plainly or with Zstd supercompression. Anything else is refused with the
/// reason: Basis Universal (BasisLZ/ETC1S and UASTC, `vkFormat` undefined) needs a transcoder this
/// engine does not have (docs/spec/texture-formats.md 7).
pub fn read(bytes: &[u8]) -> Result<Ktx2Image, String> {
    let reader =
        ktx2::Reader::new(bytes).map_err(|e| format!("not a readable KTX2 file ({e:?})"))?;
    let h = reader.header();
    let Some(vk) = h.format else {
        return Err(
            "a Basis Universal KTX2 (BasisLZ or UASTC) needs transcoding, which is not \
                    supported: write BC7 (or BC1, BC3, BC4, BC5, RGBA8) with Zstd instead"
                .into(),
        );
    };
    let (format, srgb) = format_of(vk.value()).ok_or_else(|| {
        format!("KTX2 format {vk:?} is not supported (RGBA8, BC1, BC3, BC4 unorm, BC5 unorm, BC7)")
    })?;
    if h.pixel_height == 0 {
        return Err("a 1D KTX2 texture is not supported".into());
    }
    if h.pixel_depth != 0 {
        return Err("a 3D KTX2 texture is not supported".into());
    }
    if h.layer_count > 1 {
        return Err(format!(
            "a KTX2 array texture ({} layers) is not supported",
            h.layer_count
        ));
    }
    if h.face_count != 1 {
        return Err("a KTX2 cube map is not supported".into());
    }
    let (width, height) = (h.pixel_width, h.pixel_height);
    if width > MAX_SIDE || height > MAX_SIDE {
        return Err(format!(
            "{width} x {height} is larger than {MAX_SIDE} on a side"
        ));
    }
    let zstd = match h.supercompression_scheme {
        None => false,
        Some(ktx2::SupercompressionScheme::Zstandard) => true,
        Some(s) => {
            return Err(format!(
                "KTX2 supercompression {s:?} is not supported (none or Zstandard)"
            ));
        }
    };
    let count = h.level_count.max(1);
    if count > full_chain(width, height) {
        return Err(format!(
            "{count} levels for a {width} x {height} image (a full chain has {})",
            full_chain(width, height)
        ));
    }
    let mut levels = Vec::with_capacity(count as usize);
    for (k, level) in reader.levels().enumerate() {
        let (w, hh) = level_size(width, height, k as u32);
        let want = format.level_bytes(w, hh);
        let data = if zstd {
            let mut out = Vec::with_capacity(want);
            ruzstd::decoding::StreamingDecoder::new(level.data)
                .map_err(|e| format!("level {k}: not a Zstandard frame ({e})"))?
                .take(want as u64 + 1)
                .read_to_end(&mut out)
                .map_err(|e| format!("level {k}: Zstandard decoding failed ({e})"))?;
            out
        } else {
            level.data.to_vec()
        };
        if data.len() != want {
            return Err(format!(
                "level {k} holds {} bytes; {w} x {hh} of {} takes {want}",
                data.len(),
                format.name()
            ));
        }
        levels.push(data);
    }
    Ok(Ktx2Image {
        width,
        height,
        srgb,
        levels: TextureLevels { format, levels },
    })
}

/// Decodes one `width` x `height` level of `format` to RGBA8. BC4 is read as a grey image
/// `(r, r, r, 1)` and BC5 as `(r, g, 0, 1)`, as the importer reads one- and two-channel PNGs;
/// BC1 as WebGPU's bc1-rgba.
pub fn decode(
    format: BlockFormat,
    width: u32,
    height: u32,
    data: &[u8],
) -> Result<Vec<u8>, String> {
    let (w, h) = (width as usize, height as usize);
    if data.len() < format.level_bytes(width, height) {
        return Err(format!(
            "{} bytes for a {width} x {height} level of {} ({} needed)",
            data.len(),
            format.name(),
            format.level_bytes(width, height)
        ));
    }
    if format == BlockFormat::Rgba8 {
        return Ok(data[..w * h * 4].to_vec());
    }
    let mut px = vec![0u32; w * h];
    let decoded = match format {
        BlockFormat::Rgba8 => unreachable!(),
        BlockFormat::Bc1 => texture2ddecoder::decode_bc1a(data, w, h, &mut px),
        BlockFormat::Bc3 => texture2ddecoder::decode_bc3(data, w, h, &mut px),
        BlockFormat::Bc4 => texture2ddecoder::decode_bc4(data, w, h, &mut px),
        BlockFormat::Bc5 => texture2ddecoder::decode_bc5(data, w, h, &mut px),
        BlockFormat::Bc7 => texture2ddecoder::decode_bc7(data, w, h, &mut px),
    };
    decoded.map_err(|e| format!("{}: {e}", format.name()))?;
    let mut out = Vec::with_capacity(w * h * 4);
    for p in px {
        // texture2ddecoder writes each texel as the bytes B, G, R, A.
        let [b, g, r, a] = p.to_le_bytes();
        out.extend_from_slice(&match format {
            BlockFormat::Bc4 => [r, r, r, 255],
            BlockFormat::Bc5 => [r, g, 0, 255],
            _ => [r, g, b, a],
        });
    }
    Ok(out)
}

/// A KTX2 file as an imported image: level 0 decoded to RGBA8 (`rgba8`) and the file's levels kept
/// (`levels`) for the renderer to upload as they are where it can.
pub fn image_data(name: String, bytes: &[u8], srgb: bool) -> Result<ImageData, String> {
    let image = read(bytes)?;
    let rgba8 = decode(
        image.levels.format,
        image.width,
        image.height,
        &image.levels.levels[0],
    )?;
    Ok(ImageData {
        name,
        width: image.width,
        height: image.height,
        rgba8,
        srgb,
        nearest: false,
        levels: Some(image.levels),
    })
}

/// Writes a KTX2 file of one 2D image: `levels.levels[k]` is level `k`, `srgb` picks the format's
/// sRGB variant where it has one, `zstd` supercompresses every level (Zstandard's fastest level).
/// Levels are stored smallest first, aligned as KTX 2.0 says, with a basic data format descriptor
/// and a `KTXwriter` entry.
pub fn write(width: u32, height: u32, srgb: bool, levels: &TextureLevels, zstd: bool) -> Vec<u8> {
    let format = vk_format(levels.format, srgb);
    let (mut basic, type_size) =
        ktx2::dfd::Basic::from_format(format).expect("a basic descriptor for the formats written");
    if zstd {
        // KTX 2.0: a supercompressed image's descriptor leaves its plane sizes unspecified.
        basic.bytes_planes = [0; 8];
    }
    let block = ktx2::dfd::Block::Basic(basic).to_vec();
    let mut dfd = ((block.len() + 4) as u32).to_le_bytes().to_vec();
    dfd.extend_from_slice(&block);
    let mut kvd = Vec::new();
    let entry = b"KTXwriter\0amoris ktx2\0";
    kvd.extend_from_slice(&(entry.len() as u32).to_le_bytes());
    kvd.extend_from_slice(entry);
    while !kvd.len().is_multiple_of(4) {
        kvd.push(0);
    }
    let data: Vec<Vec<u8>> = levels
        .levels
        .iter()
        .map(|l| {
            if zstd {
                ruzstd::encoding::compress_to_vec(
                    &l[..],
                    ruzstd::encoding::CompressionLevel::Fastest,
                )
            } else {
                l.clone()
            }
        })
        .collect();
    let count = data.len();
    let index_at = ktx2::Header::LENGTH;
    let dfd_at = index_at + count * ktx2::LevelIndex::LENGTH;
    let kvd_at = dfd_at + dfd.len();
    let mut out = vec![0u8; kvd_at + kvd.len()];
    // Level data: smallest level first, each at a multiple of lcm(block bytes, 4) unless
    // supercompressed.
    let align = if zstd {
        1
    } else {
        levels.format.block_bytes().max(4)
    };
    let mut index = vec![
        ktx2::LevelIndex {
            byte_offset: 0,
            byte_length: 0,
            uncompressed_byte_length: 0
        };
        count
    ];
    for k in (0..count).rev() {
        while !out.len().is_multiple_of(align) {
            out.push(0);
        }
        index[k] = ktx2::LevelIndex {
            byte_offset: out.len() as u64,
            byte_length: data[k].len() as u64,
            uncompressed_byte_length: levels.levels[k].len() as u64,
        };
        out.extend_from_slice(&data[k]);
    }
    let header = ktx2::Header {
        format: Some(format),
        type_size,
        pixel_width: width,
        pixel_height: height,
        pixel_depth: 0,
        layer_count: 0,
        face_count: 1,
        level_count: count as u32,
        supercompression_scheme: zstd.then_some(ktx2::SupercompressionScheme::Zstandard),
        index: ktx2::Index {
            dfd_byte_offset: dfd_at as u32,
            dfd_byte_length: dfd.len() as u32,
            kvd_byte_offset: kvd_at as u32,
            kvd_byte_length: kvd.len() as u32,
            sgd_byte_offset: 0,
            sgd_byte_length: 0,
        },
    };
    out[..index_at].copy_from_slice(&header.as_bytes());
    for (k, l) in index.iter().enumerate() {
        let at = index_at + k * ktx2::LevelIndex::LENGTH;
        out[at..at + ktx2::LevelIndex::LENGTH].copy_from_slice(&l.as_bytes());
    }
    out[dfd_at..kvd_at].copy_from_slice(&dfd);
    out[kvd_at..kvd_at + kvd.len()].copy_from_slice(&kvd);
    out
}

/// RGBA8 levels down to 1 x 1, each the 2 x 2 box average of the one above (in the stored
/// values, sRGB-encoded or not): what the fixture tool and the tests make files from.
pub fn box_chain(width: u32, height: u32, rgba8: &[u8]) -> Vec<Vec<u8>> {
    let mut out = vec![rgba8.to_vec()];
    let (mut w, mut h) = (width, height);
    while w > 1 || h > 1 {
        let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
        let src = out.last().expect("level 0");
        let mut next = vec![0u8; (nw * nh * 4) as usize];
        for y in 0..nh {
            for x in 0..nw {
                for c in 0..4 {
                    let at = |sx: u32, sy: u32| {
                        u32::from(src[((sy.min(h - 1) * w + sx.min(w - 1)) * 4 + c) as usize])
                    };
                    let sum = at(2 * x, 2 * y)
                        + at(2 * x + 1, 2 * y)
                        + at(2 * x, 2 * y + 1)
                        + at(2 * x + 1, 2 * y + 1);
                    next[((y * nw + x) * 4 + c) as usize] = ((sum + 2) / 4) as u8;
                }
            }
        }
        out.push(next);
        (w, h) = (nw, nh);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: u32, h: u32) -> Vec<u8> {
        let mut px = Vec::new();
        for y in 0..h {
            for x in 0..w {
                px.extend_from_slice(&[
                    (x * 255 / w.max(1)) as u8,
                    (y * 255 / h.max(1)) as u8,
                    77,
                    200,
                ]);
            }
        }
        px
    }

    /// A BC1 block of two 565 endpoints, every texel taking index `i`.
    fn bc1_block(c0: u16, c1: u16, i: u8) -> [u8; 8] {
        let bits = (0..4).fold(0u8, |a, k| a | (i << (2 * k)));
        let [a, b] = c0.to_le_bytes();
        let [c, d] = c1.to_le_bytes();
        [a, b, c, d, bits, bits, bits, bits]
    }

    #[test]
    fn rgba8_levels_round_trip_plain_and_with_zstd() {
        let (w, h) = (16, 8);
        let levels = TextureLevels {
            format: BlockFormat::Rgba8,
            levels: box_chain(w, h, &gradient(w, h)),
        };
        assert_eq!(levels.levels.len(), 5);
        for zstd in [false, true] {
            let file = write(w, h, true, &levels, zstd);
            assert!(is_ktx2(&file));
            let back = read(&file).unwrap();
            assert_eq!((back.width, back.height, back.srgb), (w, h, true));
            assert_eq!(back.levels, levels, "zstd: {zstd}");
            let image = image_data("t".into(), &file, false).unwrap();
            assert_eq!(image.rgba8, levels.levels[0]);
            assert!(!image.srgb);
        }
    }

    #[test]
    fn a_zstd_file_of_repeated_blocks_is_small() {
        let block = bc1_block(0xF800, 0x001F, 0);
        let level: Vec<u8> = block
            .iter()
            .copied()
            .cycle()
            .take(256 * 256 / 16 * 8)
            .collect();
        let levels = TextureLevels {
            format: BlockFormat::Bc1,
            levels: vec![level],
        };
        let plain = write(256, 256, false, &levels, false);
        let packed = write(256, 256, false, &levels, true);
        assert!(
            packed.len() * 10 < plain.len(),
            "{} vs {}",
            packed.len(),
            plain.len()
        );
        assert_eq!(read(&packed).unwrap().levels, levels);
    }

    #[test]
    fn bc_blocks_decode_to_their_colors_and_channels() {
        // Red and blue endpoints: index 0 red, 1 blue, 2 two thirds red (4-color mode, c0 > c1).
        let red_blue = bc1_block(0xF800, 0x001F, 0);
        assert_eq!(
            &decode(BlockFormat::Bc1, 4, 4, &red_blue).unwrap()[..4],
            &[255, 0, 0, 255]
        );
        let third = decode(BlockFormat::Bc1, 4, 4, &bc1_block(0xF800, 0x001F, 2)).unwrap();
        assert_eq!(&third[..4], &[170, 0, 85, 255]);
        // 3-color mode (c0 <= c1): index 3 is transparent black, as WebGPU's bc1-rgba.
        let punch = decode(BlockFormat::Bc1, 4, 4, &bc1_block(0x001F, 0xF800, 3)).unwrap();
        assert_eq!(&punch[..4], &[0, 0, 0, 0]);
        // BC4: a grey image; BC5: red and green; endpoints 200 and 40, index 0 everywhere.
        let bc4 = [200u8, 40, 0, 0, 0, 0, 0, 0];
        assert_eq!(
            &decode(BlockFormat::Bc4, 4, 4, &bc4).unwrap()[..4],
            &[200, 200, 200, 255]
        );
        let bc5 = [200u8, 40, 0, 0, 0, 0, 0, 0, 90, 10, 0, 0, 0, 0, 0, 0];
        assert_eq!(
            &decode(BlockFormat::Bc5, 4, 4, &bc5).unwrap()[..4],
            &[200, 90, 0, 255]
        );
        // A level smaller than a block decodes its texels only.
        assert_eq!(
            decode(BlockFormat::Bc1, 2, 1, &red_blue).unwrap(),
            [255, 0, 0, 255, 255, 0, 0, 255]
        );
        assert!(decode(BlockFormat::Bc7, 8, 4, &[0; 16]).is_err());
    }

    #[test]
    fn what_is_not_supported_is_refused_with_the_reason() {
        let levels = TextureLevels {
            format: BlockFormat::Rgba8,
            levels: box_chain(4, 4, &gradient(4, 4)),
        };
        let good = write(4, 4, false, &levels, false);
        let refused = |patch: &dyn Fn(&mut Vec<u8>)| {
            let mut f = good.clone();
            patch(&mut f);
            read(&f).unwrap_err()
        };
        let set = |at: usize, v: u32| {
            move |f: &mut Vec<u8>| f[at..at + 4].copy_from_slice(&v.to_le_bytes())
        };
        assert!(refused(&set(12, 0)).contains("Basis"));
        assert!(refused(&set(12, 147)).contains("not supported"), "ETC2");
        assert!(refused(&set(28, 4)).contains("3D"));
        assert!(refused(&set(32, 6)).contains("array"));
        assert!(refused(&set(36, 6)).contains("cube"));
        assert!(refused(&set(44, 3)).contains("supercompression"), "ZLIB");
        assert!(refused(&set(24, 0)).contains("1D"));
        assert!(refused(&set(20, 8)).contains("level 0 holds"));
        assert!(refused(&|f: &mut Vec<u8>| f.truncate(100)).contains("not a readable"));
        assert!(read(b"\x89PNG\r\n\x1a\n").is_err());
        assert!(!is_ktx2(b"\x89PNG\r\n\x1a\n"));
    }
}
