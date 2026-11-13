//! KTX2 files for tests and projects (docs/spec/texture-formats.md 6).
//!
//!     cargo run --release -p amoris-render --example ktx2 -- encode IN.png OUT.ktx2
//!         [--format bc7|bc1|bc3|bc4|bc5|rgba8] [--srgb] [--no-mips] [--no-zstd]
//!     cargo run --release -p amoris-render --example ktx2 -- fixture DIR
//!
//! `encode` writes a PNG as a KTX2 file: its full chain of 2 x 2 box averages (unless `--no-mips`),
//! each level block-compressed by Intel's ISPC texture compressor (BC7 by default), Zstd
//! supercompressed (unless `--no-zstd`), the format's sRGB variant with `--srgb` (base color and
//! emissive images; glTF's use of an image decides how the engine reads it either way).
//!
//! `fixture` writes `DIR/models/panels.glb`, the KTX2 scene fixture's model
//! (tests/fixtures/scenes/ktx2, whose `build_scene.py` runs it): four 1 m quads, each a mesh with
//! a material of its own whose textures are KTX2 images named by `KHR_texture_basisu`:
//! `bc7` (a 1024 x 1024 BC7 base color and normal map with full chains: sampled as stored where
//! the device has BC), `bc1` (a 1024 x 1024 BC1 base color: decoded on the CPU, the file's levels
//! written to the layer), `small` (a 256 x 256 BC7 base color: decoded and resized as a PNG is),
//! and `png` (the same pattern as a PNG, the reference).

#[path = "encode.rs"]
mod encode;

use amoris_assets::ktx2;
use amoris_assets::mesh::BlockFormat;
use serde_json::{Value, json};

fn format_named(name: &str) -> Option<BlockFormat> {
    Some(match name {
        "rgba8" => BlockFormat::Rgba8,
        "bc1" => BlockFormat::Bc1,
        "bc3" => BlockFormat::Bc3,
        "bc4" => BlockFormat::Bc4,
        "bc5" => BlockFormat::Bc5,
        "bc7" => BlockFormat::Bc7,
        _ => return None,
    })
}

fn usage() -> ! {
    eprintln!(
        "usage: ktx2 encode IN.png OUT.ktx2 [--format bc7|bc1|bc3|bc4|bc5|rgba8] [--srgb] \
         [--no-mips] [--no-zstd]\n       ktx2 fixture DIR"
    );
    std::process::exit(2);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("encode") if args.len() >= 3 => encode_png(&args[1], &args[2], &args[3..]),
        Some("fixture") if args.len() == 2 => fixture(std::path::Path::new(&args[1])),
        _ => usage(),
    }
}

fn encode_png(input: &str, output: &str, flags: &[String]) {
    let mut format = BlockFormat::Bc7;
    let (mut srgb, mut mips, mut zstd) = (false, true, true);
    let mut it = flags.iter();
    while let Some(f) = it.next() {
        match f.as_str() {
            "--format" => {
                format = it
                    .next()
                    .and_then(|n| format_named(n))
                    .unwrap_or_else(|| usage())
            }
            "--srgb" => srgb = true,
            "--no-mips" => mips = false,
            "--no-zstd" => zstd = false,
            _ => usage(),
        }
    }
    let img = image::open(input)
        .unwrap_or_else(|e| panic!("{input}: {e}"))
        .to_rgba8();
    let (w, h) = img.dimensions();
    let levels = encode::encode(format, w, h, img.as_raw(), mips);
    let file = ktx2::write(w, h, srgb, &levels, zstd);
    std::fs::write(output, &file).unwrap_or_else(|e| panic!("{output}: {e}"));
    println!(
        "{output}: {w} x {h} {}, {} levels, {} bytes",
        format.name(),
        levels.levels.len(),
        file.len()
    );
    // What reading it costs the engine: the file read (Zstd undone), level 0 decoded as the
    // importer does, and the other levels decoded as a device without BC does (texture-formats.md
    // 4), each the fastest of five runs.
    let best = |f: &dyn Fn()| {
        (0..5)
            .map(|_| {
                let t = std::time::Instant::now();
                f();
                t.elapsed().as_secs_f64() * 1000.0
            })
            .fold(f64::INFINITY, f64::min)
    };
    let read = ktx2::read(&file).expect("the file reads back");
    let level = |k: usize| {
        let (lw, lh) = ktx2::level_size(w, h, k as u32);
        ktx2::decode(format, lw, lh, &read.levels.levels[k]).expect("it decodes");
    };
    println!(
        "read {:.2} ms, level 0 decoded {:.2} ms, levels 1.. decoded {:.2} ms",
        best(&|| {
            ktx2::read(&file).expect("the file reads back");
        }),
        best(&|| level(0)),
        best(&|| (1..read.levels.levels.len()).for_each(level)),
    );
}

/// A KTX2 file of `pixels` (`side` square) as `format`, its full chain, Zstd.
fn ktx2_of(format: BlockFormat, side: u32, pixels: &[u8], srgb: bool) -> Vec<u8> {
    ktx2::write(
        side,
        side,
        srgb,
        &encode::encode(format, side, side, pixels, true),
        true,
    )
}

fn png_of(side: u32, pixels: &[u8]) -> Vec<u8> {
    let img = image::RgbaImage::from_raw(side, side, pixels.to_vec()).expect("an image");
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png)
        .expect("a PNG");
    out.into_inner()
}

fn fixture(dir: &std::path::Path) {
    let big = 1024;
    let color = encode::pattern(big, big, 0);
    let images: Vec<(Vec<u8>, &str)> = vec![
        (ktx2_of(BlockFormat::Bc7, big, &color, true), "image/ktx2"),
        (
            ktx2_of(BlockFormat::Bc7, big, &encode::normals(big, big), false),
            "image/ktx2",
        ),
        (
            ktx2_of(BlockFormat::Bc1, big, &encode::pattern(big, big, 1), true),
            "image/ktx2",
        ),
        (
            ktx2_of(BlockFormat::Bc7, 256, &encode::pattern(256, 256, 2), true),
            "image/ktx2",
        ),
        (png_of(256, &encode::pattern(256, 256, 2)), "image/png"),
    ];
    // Textures: a KTX2 image each, and the PNG plainly.
    let textures = json!([
        {"extensions": {"KHR_texture_basisu": {"source": 0}}},
        {"extensions": {"KHR_texture_basisu": {"source": 1}}},
        {"extensions": {"KHR_texture_basisu": {"source": 2}}},
        {"extensions": {"KHR_texture_basisu": {"source": 3}}},
        {"source": 4}
    ]);
    let material = |name: &str, base: usize, normal: Option<usize>| {
        let mut m = json!({"name": name, "pbrMetallicRoughness": {
            "baseColorTexture": {"index": base}, "metallicFactor": 0.0, "roughnessFactor": 0.8}});
        if let Some(n) = normal {
            m["normalTexture"] = json!({"index": n});
        }
        m
    };
    let names = ["bc7", "bc1", "small", "png"];
    let materials = json!([
        material("bc7", 0, Some(1)),
        material("bc1", 2, None),
        material("small", 3, None),
        material("png", 4, None)
    ]);
    let glb = glb(&images, &names, textures, materials);
    let models = dir.join("models");
    std::fs::create_dir_all(&models).expect("the models folder");
    let path = models.join("panels.glb");
    std::fs::write(&path, &glb).expect("panels.glb");
    println!("{}: {} bytes", path.display(), glb.len());
}

/// A binary glTF of one quad mesh per name (1 m square in x and y, facing +z, its texture
/// coordinates over the whole image), mesh `i` drawn with material `i`, a node each at the origin
/// (the scene places them).
fn glb(images: &[(Vec<u8>, &str)], names: &[&str], textures: Value, materials: Value) -> Vec<u8> {
    let mut bin: Vec<u8> = Vec::new();
    let mut views = Vec::new();
    let mut push = |bin: &mut Vec<u8>, bytes: &[u8], target: Option<u32>| {
        while !bin.len().is_multiple_of(4) {
            bin.push(0);
        }
        let mut v = json!({"buffer": 0, "byteOffset": bin.len(), "byteLength": bytes.len()});
        if let Some(t) = target {
            v["target"] = json!(t);
        }
        bin.extend_from_slice(bytes);
        views.push(v);
        views.len() - 1
    };
    let f32s = |v: &[f32]| v.iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>();
    let positions = f32s(&[
        -0.5, -0.5, 0.0, 0.5, -0.5, 0.0, 0.5, 0.5, 0.0, -0.5, 0.5, 0.0,
    ]);
    let normals = f32s(&[0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0]);
    let uvs = f32s(&[0.0, 1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0]);
    let indices: Vec<u8> = [0u16, 1, 2, 0, 2, 3]
        .iter()
        .flat_map(|i| i.to_le_bytes())
        .collect();
    let p = push(&mut bin, &positions, Some(34962));
    let n = push(&mut bin, &normals, Some(34962));
    let t = push(&mut bin, &uvs, Some(34962));
    let i = push(&mut bin, &indices, Some(34963));
    let image_views: Vec<usize> = images
        .iter()
        .map(|(bytes, _)| push(&mut bin, bytes, None))
        .collect();
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
    let accessors = json!([
        {"bufferView": p, "componentType": 5126, "count": 4, "type": "VEC3",
            "min": [-0.5, -0.5, 0.0], "max": [0.5, 0.5, 0.0]},
        {"bufferView": n, "componentType": 5126, "count": 4, "type": "VEC3"},
        {"bufferView": t, "componentType": 5126, "count": 4, "type": "VEC2"},
        {"bufferView": i, "componentType": 5123, "count": 6, "type": "SCALAR"}
    ]);
    let meshes: Vec<Value> = names
        .iter()
        .enumerate()
        .map(|(m, name)| {
            json!({"name": name, "primitives": [{
                "attributes": {"POSITION": 0, "NORMAL": 1, "TEXCOORD_0": 2},
                "indices": 3, "material": m}]})
        })
        .collect();
    let nodes: Vec<Value> = names
        .iter()
        .enumerate()
        .map(|(m, name)| json!({"name": name, "mesh": m}))
        .collect();
    let doc = json!({
        "asset": {"version": "2.0", "generator": "amoris ktx2 example"},
        "extensionsUsed": ["KHR_texture_basisu"],
        "extensionsRequired": ["KHR_texture_basisu"],
        "scene": 0,
        "scenes": [{"nodes": (0..names.len()).collect::<Vec<_>>()}],
        "nodes": nodes,
        "meshes": meshes,
        "materials": materials,
        "textures": textures,
        "images": images.iter().zip(&image_views).map(|((_, mime), v)| {
            json!({"bufferView": v, "mimeType": mime})
        }).collect::<Vec<_>>(),
        "accessors": accessors,
        "bufferViews": views,
        "buffers": [{"byteLength": bin.len()}],
    });
    let mut text = doc.to_string().into_bytes();
    while !text.len().is_multiple_of(4) {
        text.push(b' ');
    }
    let mut out = Vec::new();
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&((12 + 8 + text.len() + 8 + bin.len()) as u32).to_le_bytes());
    out.extend_from_slice(&(text.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&text);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&bin);
    out
}
