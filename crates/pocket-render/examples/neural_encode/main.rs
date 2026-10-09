//! The neural texture encoder (charter 4.4; docs/spec/neural-textures.md): trains a material's
//! channels into a `.ntex` on the GPU (pocket_render::neural::train), decodes it back through the
//! runtime decoder, and reports quality and size against uncompressed 8-bit storage and two BCn
//! baselines, per channel group and mip.
//!
//! ```text
//! cargo run --release -p pocket-render --example neural_encode -- [options] SOURCE...
//!   SOURCE: procedural:NAME[@SIZE]  (bricks, metal, tiles, wood, noise; SIZE default 1024)
//!           image:PATH[@SIZE]       a photo or render as base color, height from its luminance
//!           gltf:PATH#MATERIAL      a glTF material's textures
//!   --out DIR            where .ntex, .json and preview .png go (default out/neural)
//!   --steps N --batch N --seed N --lr-mlp X --lr-latent X
//!   --fine 8x8|16x4 --coarse 8x8|16x4 --shift S --sampling bilinear|taps4 --pe N --hidden A,B
//!   --no-bcn             skip the BCn baselines   --no-preview   no PNGs
//! ```
//!
//! Environment: POCKET_BACKEND and POCKET_ADAPTER pick the GPU as everywhere.

mod bcn;
mod procedural;

use std::path::{Path, PathBuf};
use std::time::Instant;

use pocket_assets::neural::{
    Channel, ChannelGroup, GridSpec, NeuralLayout, NeuralTexture, Sampling, full_mip_count,
    mip_size,
};
use pocket_render::neural::train::{Reference, TrainConfig, Trainer};
use pocket_render::neural::{decode_on_gpu, describe};
use pocket_render::{BackendChoice, Gpu};
use serde_json::{Value, json};

use bcn::Format;
use procedural::Material;

struct Options {
    out: PathBuf,
    config: TrainConfig,
    layout: NeuralLayout,
    bcn: bool,
    preview: bool,
    sources: Vec<String>,
}

fn grid(s: &str) -> GridSpec {
    match s {
        "8x8" => GridSpec {
            features: 8,
            bits: 8,
        },
        "16x4" => GridSpec {
            features: 16,
            bits: 4,
        },
        _ => panic!("a grid is 8x8 or 16x4, not {s}"),
    }
}

fn parse() -> Options {
    let mut o = Options {
        out: PathBuf::from("out/neural"),
        config: TrainConfig::default(),
        layout: NeuralLayout {
            channels: Vec::new(),
            fine: grid("8x8"),
            coarse: grid("8x8"),
            fine_shift: 2,
            sampling: Sampling::Bilinear,
            pe_octaves: 2,
            hidden: [32, 32],
        },
        bcn: true,
        preview: true,
        sources: Vec::new(),
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        let mut value = || {
            i += 1;
            args.get(i)
                .unwrap_or_else(|| panic!("{a} needs a value"))
                .clone()
        };
        match a {
            "--out" => o.out = PathBuf::from(value()),
            "--steps" => o.config.steps = value().parse().expect("--steps"),
            "--batch" => o.config.batch = value().parse().expect("--batch"),
            "--seed" => o.config.seed = value().parse().expect("--seed"),
            "--lr-mlp" => o.config.lr_mlp = value().parse().expect("--lr-mlp"),
            "--lr-latent" => o.config.lr_latent = value().parse().expect("--lr-latent"),
            "--center" => o.config.center_fraction = value().parse().expect("--center"),
            "--mip-decay" => o.config.mip_decay = value().parse().expect("--mip-decay"),
            "--fine" => o.layout.fine = grid(&value()),
            "--coarse" => o.layout.coarse = grid(&value()),
            "--shift" => o.layout.fine_shift = value().parse().expect("--shift"),
            "--pe" => o.layout.pe_octaves = value().parse().expect("--pe"),
            "--sampling" => {
                o.layout.sampling = match value().as_str() {
                    "bilinear" => Sampling::Bilinear,
                    "taps4" => Sampling::Taps4,
                    s => panic!("--sampling is bilinear or taps4, not {s}"),
                }
            }
            "--hidden" => {
                let v: Vec<u32> = value()
                    .split(',')
                    .map(|s| s.parse().expect("--hidden A,B"))
                    .collect();
                o.layout.hidden = [v[0], *v.get(1).unwrap_or(&v[0])];
            }
            "--no-bcn" => o.bcn = false,
            "--no-preview" => o.preview = false,
            s if s.starts_with("--") => panic!("unknown option {s}"),
            s => o.sources.push(s.to_owned()),
        }
        i += 1;
    }
    if o.sources.is_empty() {
        eprintln!(
            "usage: neural_encode [options] procedural:NAME[@SIZE] | image:PATH[@SIZE] | gltf:PATH#MATERIAL ..."
        );
        std::process::exit(2);
    }
    o
}

fn with_size(s: &str, default: u32) -> (&str, u32) {
    match s.rsplit_once('@') {
        Some((a, n)) => (a, n.parse().expect("@SIZE")),
        None => (s, default),
    }
}

/// A glTF material's textures as channels, every image resized to the largest's power-of-two size.
fn from_gltf(spec: &str) -> Result<Material, String> {
    let (path, name) = spec
        .split_once('#')
        .ok_or("gltf:PATH#MATERIAL needs a material")?;
    let asset = pocket_assets::import::import_gltf(Path::new(path)).map_err(|p| p.message)?;
    let m = asset
        .materials
        .iter()
        .find(|m| m.name == name)
        .ok_or_else(|| format!("{path} has no material {name}"))?;
    let mut sources: Vec<(usize, Vec<(Channel, usize)>)> = Vec::new();
    if let Some(t) = m.base_color_texture {
        sources.push((
            t,
            vec![
                (Channel::BaseR, 0),
                (Channel::BaseG, 1),
                (Channel::BaseB, 2),
            ],
        ));
    }
    if let Some(t) = m.normal_texture {
        sources.push((t, vec![(Channel::NormalX, 0), (Channel::NormalY, 1)]));
    }
    if let Some(t) = m.occlusion_texture {
        sources.push((t, vec![(Channel::Occlusion, 0)]));
    }
    if let Some(t) = m.metallic_roughness_texture {
        sources.push((t, vec![(Channel::Roughness, 1), (Channel::Metallic, 2)]));
    }
    if let Some(t) = m.emissive_texture {
        sources.push((
            t,
            vec![
                (Channel::EmissiveR, 0),
                (Channel::EmissiveG, 1),
                (Channel::EmissiveB, 2),
            ],
        ));
    }
    if sources.is_empty() {
        return Err(format!("material {name} has no textures"));
    }
    let size = sources
        .iter()
        .map(|(t, _)| asset.images[*t].width.max(asset.images[*t].height))
        .max()
        .unwrap()
        .next_power_of_two();
    let mut channels: Vec<(Channel, Vec<f32>)> = Vec::new();
    for (t, picks) in &sources {
        let img = &asset.images[*t];
        let rgba = image::RgbaImage::from_raw(img.width, img.height, img.rgba8.clone())
            .ok_or("bad image")?;
        let rgba =
            image::imageops::resize(&rgba, size, size, image::imageops::FilterType::Triangle);
        for &(c, at) in picks {
            channels.push((c, rgba.pixels().map(|p| p[at] as f32 / 255.0).collect()));
        }
    }
    channels.sort_by_key(|(c, _)| *c);
    let n = (size * size) as usize;
    let mut texels = Vec::with_capacity(n * channels.len());
    for t in 0..n {
        for (_, v) in &channels {
            texels.push(v[t]);
        }
    }
    Ok(Material {
        name: name.to_owned(),
        size,
        channels: channels.into_iter().map(|(c, _)| c).collect(),
        texels,
    })
}

fn source(s: &str) -> Material {
    let m = if let Some(rest) = s.strip_prefix("procedural:") {
        let (name, size) = with_size(rest, 1024);
        procedural::material(name, size).ok_or_else(|| {
            format!(
                "no procedural material {name}; have {:?}",
                procedural::NAMES
            )
        })
    } else if let Some(rest) = s.strip_prefix("image:") {
        let (path, size) = with_size(rest, 1024);
        procedural::from_image(path, size)
    } else if let Some(rest) = s.strip_prefix("gltf:") {
        from_gltf(rest)
    } else {
        Err(format!("unknown source {s}"))
    };
    m.unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1)
    })
}

/// The channel groups present, each with its channels' indices.
fn groups(channels: &[Channel]) -> Vec<(ChannelGroup, Vec<usize>)> {
    let mut out: Vec<(ChannelGroup, Vec<usize>)> = Vec::new();
    for (i, c) in channels.iter().enumerate() {
        match out.iter_mut().find(|(g, _)| *g == c.group()) {
            Some((_, v)) => v.push(i),
            None => out.push((c.group(), vec![i])),
        }
    }
    out
}

/// The BCn format each baseline gives a group.
fn bc_format(group: ChannelGroup, high: bool) -> Format {
    match (group, high) {
        (ChannelGroup::Normal, _) => Format::Bc5,
        (ChannelGroup::Height, _) => Format::Bc4,
        (_, true) => Format::Bc7,
        (_, false) => Format::Bc1,
    }
}

/// ORM keeps glTF's slots (R occlusion, G roughness, B metallic) for BCn, missing ones zero.
fn bc_slots(group: ChannelGroup, channels: &[Channel], idx: &[usize]) -> Vec<Option<usize>> {
    if group != ChannelGroup::Orm {
        return idx.iter().map(|&i| Some(i)).collect();
    }
    [Channel::Occlusion, Channel::Roughness, Channel::Metallic]
        .iter()
        .map(|want| idx.iter().copied().find(|&i| channels[i] == *want))
        .collect()
}

fn gather(values: &[f32], c: usize, slots: &[Option<usize>]) -> Vec<f32> {
    let n = values.len() / c;
    let mut out = Vec::with_capacity(n * slots.len());
    for t in 0..n {
        for s in slots {
            out.push(s.map_or(0.0, |i| values[t * c + i]));
        }
    }
    out
}

/// PSNR (dB) of `got` against `want` over the slots that hold channels, values in [0, 1].
fn psnr(want: &[f32], got: &[f32], k: usize, slots: &[Option<usize>]) -> f64 {
    let mut sum = 0.0f64;
    let mut n = 0usize;
    for (w, g) in want.chunks(k).zip(got.chunks(k)) {
        for (j, s) in slots.iter().enumerate() {
            if s.is_some() {
                let d = f64::from(w[j]) - f64::from(g[j].clamp(0.0, 1.0));
                sum += d * d;
                n += 1;
            }
        }
    }
    let mse = sum / n.max(1) as f64;
    if mse <= 0.0 {
        99.0
    } else {
        10.0 * (1.0 / mse).log10()
    }
}

/// Mean angle (degrees) between normal maps given as (x, y) pairs in [0, 1].
fn normal_angle(want: &[f32], got: &[f32]) -> f64 {
    let n = |x: f32, y: f32| {
        let (x, y) = (
            f64::from(x.clamp(0.0, 1.0)) * 2.0 - 1.0,
            f64::from(y.clamp(0.0, 1.0)) * 2.0 - 1.0,
        );
        let z = (1.0 - x * x - y * y).max(0.0).sqrt();
        let l = (x * x + y * y + z * z).sqrt().max(1e-9);
        [x / l, y / l, z / l]
    };
    let mut sum = 0.0;
    let count = want.len() / 2;
    for i in 0..count {
        let a = n(want[2 * i], want[2 * i + 1]);
        let b = n(got[2 * i], got[2 * i + 1]);
        let d = (a[0] * b[0] + a[1] * b[1] + a[2] * b[2]).clamp(-1.0, 1.0);
        sum += d.acos().to_degrees();
    }
    sum / count.max(1) as f64
}

/// Bilinear upsampling by two with wrapping (a half-resolution texture seen at full resolution).
fn upsample(values: &[f32], k: usize, w: u32, h: u32) -> Vec<f32> {
    let (w2, h2) = (w * 2, h * 2);
    let mut out = Vec::with_capacity((w2 * h2) as usize * k);
    let at = |x: i64, y: i64, c: usize| {
        values
            [((y.rem_euclid(h as i64) as u32 * w + x.rem_euclid(w as i64) as u32) as usize) * k + c]
    };
    for y in 0..h2 {
        for x in 0..w2 {
            let px = (x as f32 + 0.5) / 2.0 - 0.5;
            let py = (y as f32 + 0.5) / 2.0 - 0.5;
            let (x0, y0) = (px.floor(), py.floor());
            let (fx, fy) = (px - x0, py - y0);
            let (x0, y0) = (x0 as i64, y0 as i64);
            for c in 0..k {
                let a = at(x0, y0, c) * (1.0 - fx) + at(x0 + 1, y0, c) * fx;
                let b = at(x0, y0 + 1, c) * (1.0 - fx) + at(x0 + 1, y0 + 1, c) * fx;
                out.push(a * (1.0 - fy) + b * fy);
            }
        }
    }
    out
}

fn quantized(values: &[f32]) -> Vec<f32> {
    values.iter().map(|v| (v * 255.0).round() / 255.0).collect()
}

/// A preview tile (128x128 from the centre of mip 0) of a group, as RGB bytes.
fn tile(values: &[f32], c: usize, size: u32, group: ChannelGroup, idx: &[usize]) -> Vec<[u8; 3]> {
    let s = 128.min(size);
    let o = (size - s) / 2;
    let mut out = Vec::new();
    for y in o..o + s {
        for x in o..o + s {
            let t = (y * size + x) as usize * c;
            let v = |i: usize| values[t + i].clamp(0.0, 1.0);
            let rgb = match group {
                ChannelGroup::Normal => {
                    let (nx, ny) = (v(idx[0]) * 2.0 - 1.0, v(idx[1]) * 2.0 - 1.0);
                    let nz = (1.0 - nx * nx - ny * ny).max(0.0).sqrt();
                    [v(idx[0]), v(idx[1]), nz * 0.5 + 0.5]
                }
                ChannelGroup::Height => [v(idx[0]); 3],
                _ => std::array::from_fn(|j| idx.get(j).map_or(0.0, |&i| v(i))),
            };
            out.push(rgb.map(|f| (f * 255.0).round() as u8));
        }
    }
    out
}

fn main() {
    let o = parse();
    std::fs::create_dir_all(&o.out).expect("output directory");
    let gpu = Gpu::headless(BackendChoice::from_env()).unwrap_or_else(|e| {
        eprintln!("no GPU: {e}");
        std::process::exit(1)
    });
    eprintln!("GPU: {} ({})", gpu.info.name, gpu.backend_name());
    for s in &o.sources {
        let material = source(s);
        let report = encode(&gpu, &o, &material);
        let path = o.out.join(format!("{}.json", material.name));
        std::fs::write(&path, serde_json::to_string_pretty(&report).unwrap()).expect("report");
        eprintln!("wrote {}", path.display());
    }
}

fn encode(gpu: &Gpu, o: &Options, m: &Material) -> Value {
    let (device, queue) = (&gpu.device, &gpu.queue);
    let size = m.size;
    let mips = full_mip_count(size, size);
    let reference =
        Reference::new(size, size, m.channels.clone(), m.texels.clone(), mips).expect("reference");
    let mut layout = o.layout.clone();
    layout.channels = m.channels.clone();
    eprintln!("{}: {size}x{size}, {}", m.name, describe(&layout));
    let c = m.channels.len();

    let started = Instant::now();
    let mut trainer = Trainer::new(device, &reference, layout.clone(), o.config.clone())
        .unwrap_or_else(|e| panic!("{}: {e}", m.name));
    trainer
        .run(
            device,
            queue,
            (o.config.steps / 10).max(1),
            &mut |step, loss| {
                eprintln!(
                    "  step {step:>6}  loss {loss:.6}  {:.1} s",
                    started.elapsed().as_secs_f64()
                );
            },
        )
        .expect("training");
    let history = trainer.history(device, queue).expect("history");
    let texture = trainer.finish(device, queue, &m.name).expect("finish");
    let train_s = started.elapsed().as_secs_f64();
    let bytes = texture.to_bytes();
    let ntex = o.out.join(format!("{}.ntex", m.name));
    std::fs::write(&ntex, &bytes).expect("write .ntex");
    assert_eq!(NeuralTexture::from_bytes(&bytes).as_ref(), Ok(&texture));

    let decoded32 = decode_on_gpu(device, queue, &texture, false).expect("decode f32");
    let decoded16 = gpu
        .caps
        .shader_f16
        .then(|| decode_on_gpu(device, queue, &texture, true).expect("decode f16"));

    let groups = groups(&m.channels);
    let texels0 = f64::from(size) * f64::from(size);
    let bpt = |bytes: usize| 8.0 * bytes as f64 / texels0;
    let mut quality = Vec::new();
    let mut bc_bytes = [0usize; 2];
    let mut previews: Vec<Vec<Vec<[u8; 3]>>> = Vec::new();
    for (group, idx) in &groups {
        let slots = bc_slots(*group, &m.channels, idx);
        let k = slots.len();
        let mut per_mip: Vec<Value> = Vec::new();
        let mut bc_mip0 = None;
        for mip in 0..mips {
            let [w, h] = mip_size(size, size, mip);
            let want = gather(&quantized(&reference.mips[mip as usize]), c, &slots);
            let got32 = gather(&decoded32[mip as usize], c, &slots);
            let mut entry = json!({
                "mip": mip,
                "size": w,
                "neural_f32": psnr(&want, &got32, k, &slots),
            });
            if let Some(d) = &decoded16 {
                entry["neural_f16"] =
                    json!(psnr(&want, &gather(&d[mip as usize], c, &slots), k, &slots));
            }
            if *group == ChannelGroup::Normal {
                entry["neural_f32_angle_deg"] = json!(normal_angle(&want, &got32));
            }
            if o.bcn {
                for (bi, high) in [true, false].into_iter().enumerate() {
                    let format = bc_format(*group, high);
                    let (dec, stored) = bcn::round_trip(&want, k, w, h, format);
                    bc_bytes[bi] += stored;
                    let key = if high { "bc_high" } else { "bc_low" };
                    entry[key] = json!(psnr(&want, &dec, k, &slots));
                    if *group == ChannelGroup::Normal {
                        entry[format!("{key}_angle_deg")] = json!(normal_angle(&want, &dec));
                    }
                    if high && mip == 1 {
                        // Half resolution, seen at mip 0's size.
                        let want0 = gather(&quantized(&reference.mips[0]), c, &slots);
                        let up = upsample(&dec, k, w, h);
                        per_mip[0]["bc_high_half_res"] = json!(psnr(&want0, &up, k, &slots));
                        if *group == ChannelGroup::Normal {
                            per_mip[0]["bc_high_half_res_angle_deg"] =
                                json!(normal_angle(&want0, &up));
                        }
                    }
                    if high && mip == 0 {
                        bc_mip0 = Some(dec);
                    }
                }
            }
            per_mip.push(entry);
        }
        if o.preview {
            let full = |v: &[f32]| tile(v, c, size, *group, idx);
            let mut row = vec![full(&quantized(&reference.mips[0])), full(&decoded32[0])];
            if let Some(dec) = bc_mip0 {
                // Back to the material's channel layout for the tile.
                let mut back = vec![0.0f32; (size * size) as usize * c];
                for t in 0..(size * size) as usize {
                    for (j, s) in slots.iter().enumerate() {
                        if let Some(i) = s {
                            back[t * c + i] = dec[t * k + j];
                        }
                    }
                }
                row.push(full(&back));
            }
            previews.push(row);
        }
        quality.push(json!({
            "group": group.name(),
            "channels": idx.len(),
            "bc_high_format": bc_format(*group, true).name(),
            "bc_low_format": bc_format(*group, false).name(),
            "mips": per_mip,
        }));
    }
    if o.preview && !previews.is_empty() {
        let s = 128.min(size);
        let cols = previews[0].len() as u32;
        let mut img = image::RgbImage::new(s * cols, s * previews.len() as u32);
        for (r, row) in previews.iter().enumerate() {
            for (col, t) in row.iter().enumerate() {
                for (i, px) in t.iter().enumerate() {
                    let (x, y) = (i as u32 % s, i as u32 / s);
                    img.put_pixel(col as u32 * s + x, r as u32 * s + y, image::Rgb(*px));
                }
            }
        }
        let path = o.out.join(format!("{}-preview.png", m.name));
        img.save(&path).expect("preview");
    }

    let tex_size = texture.size();
    let chain_texels: u64 = (0..mips)
        .map(|mip| {
            let [w, h] = mip_size(size, size, mip);
            u64::from(w) * u64::from(h)
        })
        .sum();
    let gpu_formats: usize = groups
        .iter()
        .map(|(g, _)| match g {
            ChannelGroup::Normal => 2,
            ChannelGroup::Height => 1,
            _ => 4,
        })
        .sum();
    let step_ms = 1000.0 * train_s / f64::from(o.config.steps);
    eprintln!(
        "  {:.2} bit/texel ({} bytes), trained in {train_s:.1} s ({step_ms:.2} ms/step)",
        tex_size.bits_per_texel, tex_size.file_bytes
    );
    for q in &quality {
        let mip0 = &q["mips"][0];
        eprintln!(
            "  {:<10} mip0 neural {:.2} dB (f16 {:.2})  BC high {:.2}  BC low {:.2}  BC half-res {:.2}",
            q["group"].as_str().unwrap(),
            mip0["neural_f32"].as_f64().unwrap_or(0.0),
            mip0["neural_f16"].as_f64().unwrap_or(0.0),
            mip0["bc_high"].as_f64().unwrap_or(0.0),
            mip0["bc_low"].as_f64().unwrap_or(0.0),
            mip0["bc_high_half_res"].as_f64().unwrap_or(0.0),
        );
    }
    json!({
        "material": m.name,
        "size": size,
        "mips": mips,
        "channels": m.channels.iter().map(|c| c.name()).collect::<Vec<_>>(),
        "layout": describe(&layout),
        "layout_detail": layout,
        "train": o.config,
        "gpu": gpu.info.name,
        "backend": gpu.backend_name(),
        "train_seconds": train_s,
        "train_ms_per_step": step_ms,
        "loss_first": history.first(),
        "loss_last": history.last(),
        "loss_every_100": history.iter().step_by(100).collect::<Vec<_>>(),
        "size_bytes": {
            "neural_file": tex_size.file_bytes,
            "neural_latents": tex_size.latent_bytes,
            "neural_weights": tex_size.weight_bytes,
            "uncompressed_8bit_tight": chain_texels as usize * c,
            "uncompressed_gpu_formats": chain_texels as usize * gpu_formats,
            "bc_high": bc_bytes[0],
            "bc_low": bc_bytes[1],
        },
        "bits_per_texel": {
            "neural": tex_size.bits_per_texel,
            "uncompressed_8bit_tight": bpt(chain_texels as usize * c),
            "uncompressed_gpu_formats": bpt(chain_texels as usize * gpu_formats),
            "bc_high": bpt(bc_bytes[0]),
            "bc_low": bpt(bc_bytes[1]),
        },
        "quality_psnr_db": quality,
    })
}
