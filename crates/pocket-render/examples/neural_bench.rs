//! What decoding a neural texture costs in the forward pass (docs/bench/neural-textures.md): the
//! neural texture scene (`demo::neural_scene`, a tiled ground) drawn offscreen with the material
//! four ways, the opaque pass timed by GPU timestamps:
//!
//! - `inline`: the inline material, no textures (the floor of the pass's cost);
//! - `textured`: the same material's reference channels as ordinary textures (RGBA8 arrays with
//!   mips, trilinear and anisotropic filtering: base color, normal, metallic-roughness);
//! - `neural_f16` / `neural_f32`: the `.ntex`, decoded in half or single precision.
//!
//! ```text
//! cargo run --release -p pocket-render --example neural_bench -- NTEX SOURCE
//!     [--sizes 1920x1080,2560x1440] [--frames 200] [--view 0|1] [--variants a,b]
//! ```
//!
//! SOURCE is the material the `.ntex` was trained from (`procedural:bricks@1024`, as
//! neural_encode takes it), for the textured variant. Prints one JSON object. POCKET_BACKEND and
//! POCKET_ADAPTER pick the GPU; every timing is of a loaded or quiet machine, as it was.

#[allow(dead_code)]
#[path = "neural_encode/procedural.rs"]
mod procedural;

use pocket_assets::mesh::{ImageData, MaterialData, ModelAsset};
use pocket_assets::neural::{Channel, NeuralTexture};
use pocket_render::occlusion::OcclusionMode;
use pocket_render::{BackendChoice, Gpu, Renderer, demo};
use serde_json::{Value, json};

const TEXTURED: &str = "demo/textured-ground.glb";
const NTEX: &str = "bench/material.ntex";

fn arg(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

/// The material's channels as glTF images on the ground model.
fn textured_model(m: &procedural::Material) -> ModelAsset {
    let n = (m.size * m.size) as usize;
    let c = m.channels.len();
    let at = |ch: Channel| m.channels.iter().position(|&x| x == ch);
    let image = |name: &str, srgb: bool, picks: [Option<usize>; 4], fill: [u8; 4]| ImageData {
        name: name.into(),
        width: m.size,
        height: m.size,
        rgba8: (0..n)
            .flat_map(|t| {
                std::array::from_fn::<u8, 4, _>(|k| {
                    picks[k].map_or(fill[k], |i| (m.texels[t * c + i] * 255.0).round() as u8)
                })
            })
            .collect(),
        srgb,
    };
    let mut normal = image(
        "normal",
        false,
        [at(Channel::NormalX), at(Channel::NormalY), None, None],
        [128, 128, 255, 255],
    );
    for p in normal.rgba8.chunks_mut(4) {
        let x = p[0] as f32 / 127.5 - 1.0;
        let y = p[1] as f32 / 127.5 - 1.0;
        p[2] = (((1.0 - x * x - y * y).max(0.0).sqrt() * 0.5 + 0.5) * 255.0).round() as u8;
    }
    let mut model = demo::neural_ground_model(40.0, 8.0);
    model.images = vec![
        image(
            "base",
            true,
            [
                at(Channel::BaseR),
                at(Channel::BaseG),
                at(Channel::BaseB),
                None,
            ],
            [255; 4],
        ),
        normal,
        image(
            "metal_rough",
            false,
            [None, at(Channel::Roughness), at(Channel::Metallic), None],
            [0, 255, 0, 255],
        ),
    ];
    model.materials = vec![MaterialData {
        name: "material".into(),
        base_color: [1.0; 4],
        metallic: 1.0,
        roughness: 1.0,
        base_color_texture: Some(0),
        normal_texture: Some(1),
        metallic_roughness_texture: Some(2),
        ..MaterialData::default()
    }];
    model.meshes[0].material = Some(0);
    model
}

fn summary(v: &mut [f64]) -> Value {
    v.sort_by(f64::total_cmp);
    let n = v.len().max(1);
    let round = |x: f64| (x * 1000.0).round() / 1000.0;
    json!({
        "mean": round(v.iter().sum::<f64>() / n as f64),
        "p50": round(v.get(v.len() / 2).copied().unwrap_or(0.0)),
        "p10": round(v.get(v.len() / 10).copied().unwrap_or(0.0)),
        "p90": round(v.get(v.len() * 9 / 10).copied().unwrap_or(0.0)),
    })
}

fn main() {
    let args: Vec<String> = std::env::args()
        .skip(1)
        .take_while(|a| !a.starts_with("--"))
        .collect();
    let [ntex_path, source] = args.as_slice() else {
        eprintln!("usage: neural_bench NTEX SOURCE [--sizes WxH,..] [--frames N] [--view 0|1]");
        std::process::exit(2);
    };
    let frames: u32 = arg("--frames").and_then(|s| s.parse().ok()).unwrap_or(200);
    let view: u32 = arg("--view").and_then(|s| s.parse().ok()).unwrap_or(0);
    let sizes: Vec<(u32, u32)> = arg("--sizes")
        .unwrap_or_else(|| "1920x1080,2560x1440".into())
        .split(',')
        .map(|s| {
            let (w, h) = s.split_once('x').expect("WxH");
            (w.parse().expect("width"), h.parse().expect("height"))
        })
        .collect();
    let texture = NeuralTexture::load(std::path::Path::new(ntex_path)).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1)
    });
    let spec = source
        .strip_prefix("procedural:")
        .expect("SOURCE: procedural:NAME[@SIZE]");
    let (name, size) = match spec.split_once('@') {
        Some((n, s)) => (n, s.parse().expect("@SIZE")),
        None => (spec, 1024),
    };
    let material = procedural::material(name, size).expect("a procedural material");
    let gpu = Gpu::headless(BackendChoice::from_env()).expect("gpu");
    if !gpu.caps.timestamps {
        eprintln!("this device has no timestamp queries: nothing to measure");
        std::process::exit(1);
    }
    let mut variants = vec!["inline", "textured"];
    if gpu.caps.shader_f16 {
        variants.push("neural_f16");
    }
    variants.push("neural_f32");
    if let Some(only) = arg("--variants") {
        variants.retain(|v| only.split(',').any(|o| o == *v));
    }
    let textured = textured_model(&material);
    let mut results = Vec::new();
    for &(w, h) in &sizes {
        let mut row = serde_json::Map::new();
        for &variant in &variants {
            let mut r = Renderer::new(&gpu, wgpu::TextureFormat::Rgba8UnormSrgb, w, h);
            r.set_occlusion(OcclusionMode::Off);
            r.add_model(demo::NEURAL_GROUND, &demo::neural_ground_model(40.0, 8.0));
            r.add_model(TEXTURED, &textured);
            let frame = match variant {
                "inline" => demo::neural_scene("", [0.5, 0.4, 0.3, 1.0], 0.6),
                "textured" => {
                    let mut f = demo::neural_scene("", [1.0; 4], 1.0);
                    if let Some(look) = f.instances[0].look.as_mut() {
                        look.mesh = TEXTURED.into();
                    }
                    f
                }
                _ => {
                    r.set_neural_half_precision(variant == "neural_f16");
                    r.add_neural_texture(NTEX, &texture)
                        .expect("the texture loads");
                    demo::neural_scene(NTEX, [1.0; 4], 1.0)
                }
            };
            r.apply(frame, 0.0);
            r.set_camera_override(Some(demo::neural_camera(view)));
            let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("bench target"),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            let out = target.create_view(&Default::default());
            let mut opaque = Vec::new();
            let mut total = Vec::new();
            for i in 0..60 + frames {
                let stats = r.render(&out, f64::from(i) / 60.0);
                let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
                if i < 60 || stats.passes.is_empty() {
                    continue;
                }
                total.push(f64::from(stats.gpu_ms));
                if let Some((_, ms)) = stats.passes.iter().find(|(l, _)| l.starts_with("opaque")) {
                    opaque.push(f64::from(*ms));
                }
            }
            eprintln!(
                "{w}x{h} {variant:<11} opaque {:?}",
                summary(&mut opaque.clone())["p50"]
            );
            row.insert(
                variant.into(),
                json!({"opaque_ms": summary(&mut opaque), "gpu_ms": summary(&mut total)}),
            );
        }
        results.push(json!({"size": [w, h], "variants": row}));
    }
    println!(
        "{}",
        json!({
            "bench": "neural texture decode",
            "ntex": ntex_path,
            "layout": pocket_render::neural::describe(&texture.layout),
            "texture": [texture.width, texture.height],
            "source": source,
            "view": view,
            "backend": gpu.backend_name(),
            "adapter": gpu.info.name,
            "driver": format!("{} {}", gpu.info.driver, gpu.info.driver_info),
            "frames": frames,
            "results": results,
        })
    );
}
