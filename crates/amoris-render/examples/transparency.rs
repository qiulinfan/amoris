//! The transparent stage's cost and its TAA ghosting (docs/bench/transparency.md; charter 4.4,
//! Pioneer 2026-10-16; docs/spec/transparency.md). Headless, offscreen.
//!
//! `cargo run --release -p amoris-render --example transparency -- bench [--count 1000]
//!  [--kind hidden|opaque|blend|glass|double] [--size 1600x900] [--frames 300] [--warm 60]
//!  [--settle 5] [--orbit] [--prepass off|on|auto] [--occlusion off|on|auto]`
//!
//! `bench` draws `--count` spheres in a cubic lattice above a floor, in front of a row of opaque
//! cubes, lit by a sun with shadows under the atmosphere: `hidden` (the spheres' looks invisible:
//! the scene without them), `opaque`, `blend` (a look alpha of 0.35, the inline material),
//! `glass` (transmission 1) or `double` (a double-sided BLEND material: two draws per sphere).
//! The transparent stage's GPU cost is a kind's GPU time less `hidden`'s, against `opaque`'s for
//! the same spheres drawn in the GPU-driven batches; its CPU cost is the culling and sorting
//! (`transparent_ms`) and the render call's encoding time against `hidden`'s. `--orbit` turns the
//! camera about the lattice (the order changes every frame). The depth prepass and occlusion
//! culling are off unless asked for, so the kinds differ only by the spheres. Prints one JSON
//! object, with `Renderer::new`'s time and the first frame's encoding time (pipelines made on
//! first use would show there).
//!
//! `cargo run --release -p amoris-render --example transparency -- ghost [--kind opaque|blend|glass]
//!  [--motion object|pan] [--size 960x540] [--frames 48]`
//!
//! `ghost` measures what TAA does to a transparent surface, which has no motion vectors: a sphere
//! moving across stripes (`object`), or standing while the camera pans (`pan`), drawn with TAA and
//! with 4x MSAA on one device, the same frames. After `--frames` frames it reports each image's
//! mean absolute difference (linear luminance) from a reference of that last moment held still
//! (the mean of 64 frames jittered over the pixel, as aa_eval's references), where the sphere is
//! now and in its trail (the pixels it covered in the last eight frames and does not cover now).
//! MSAA has no history: its difference is the floor; the opaque sphere, which writes motion
//! vectors, is the control. `--save DIR` writes the three images side by side.

use amoris_assets::frame::{
    CameraView, EnvironmentView, InstanceUpdate, LightKindView, LightView, Look, Pose, RenderFrame,
};
use amoris_assets::mesh::{AlphaMode, MaterialData, ModelAsset, NodeData};
use amoris_assets::primitives::primitive;
use amoris_render::{
    Antialiasing, BackendChoice, CameraState, Gpu, Gtao, OcclusionMode, PrepassMode, Renderer,
};
use glam::Vec3;

const DOUBLE: &str = "bench/double.glb";
const DT: f64 = 1.0 / 60.0;

fn arg(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

fn flag(name: &str) -> bool {
    std::env::args().any(|a| a == name)
}

fn size(default: (u32, u32)) -> (u32, u32) {
    arg("--size")
        .and_then(|s| {
            s.split_once('x')
                .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
        })
        .unwrap_or(default)
}

/// A sphere whose BLEND material is double-sided (an inline material cannot be).
fn double_model() -> ModelAsset {
    let mut mesh = primitive("sphere").expect("the sphere primitive");
    // Levels of detail as the built-in sphere has them (an asset gets them at import).
    amoris_assets::lod::build(&mut mesh, &amoris_assets::lod::LodOptions::default());
    mesh.name = "sphere".into();
    mesh.material = Some(0);
    ModelAsset {
        meshes: vec![mesh],
        materials: vec![MaterialData {
            name: "glass".into(),
            base_color: [0.6, 0.85, 1.0, 0.35],
            roughness: 0.2,
            alpha_mode: AlphaMode::Blend,
            double_sided: true,
            ..MaterialData::default()
        }],
        nodes: vec![NodeData {
            name: "sphere".into(),
            mesh: 0,
            transform: glam::Mat4::IDENTITY.to_cols_array(),
            skin: None,
        }],
        ..ModelAsset::default()
    }
}

fn look(mesh: &str, color: [f32; 4]) -> Look {
    Look {
        mesh: mesh.into(),
        material: String::new(),
        color,
        metallic: 0.0,
        roughness: 0.4,
        transmission: None,
        ior: None,
        emissive: [0.0; 3],
        cast_shadows: true,
        visible: true,
    }
}

/// The look of a sphere of `kind` with base color `rgb`.
fn sphere_look(kind: &str, rgb: [f32; 3]) -> Look {
    let [r, g, b] = rgb;
    match kind {
        "hidden" => Look {
            visible: false,
            ..look("sphere", [r, g, b, 1.0])
        },
        "opaque" => look("sphere", [r, g, b, 1.0]),
        "blend" => look("sphere", [r, g, b, 0.35]),
        "glass" => Look {
            transmission: Some(1.0),
            roughness: 0.05,
            ..look("sphere", [0.9, 0.95, 1.0, 1.0])
        },
        "double" => look(&format!("{DOUBLE}#sphere"), [1.0; 4]),
        other => panic!("unknown kind {other}"),
    }
}

fn instance(id: u64, position: [f32; 3], scale: [f32; 3], look: Look) -> InstanceUpdate {
    InstanceUpdate {
        id,
        pose: Some(Pose {
            position,
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale,
        }),
        look: Some(look),
        anim: None,
    }
}

fn environment() -> EnvironmentView {
    EnvironmentView {
        sky: 0,
        sky_color: [0.35, 0.5, 0.75],
        ambient: 1.0,
        baked_gi: String::new(),
        neural_gi: String::new(),
        gi_intensity: 1.0,
        fog_density: 0.0,
        fog_color: [0.6, 0.7, 0.8],
        exposure_ev: 0.0,
        bloom: 0.0,
    }
}

fn sun() -> LightView {
    LightView {
        id: 0,
        kind: LightKindView::Directional,
        position: [0.0; 3],
        direction: Vec3::new(-0.4, -0.8, -0.45).normalize().to_array(),
        color: [1.0, 0.96, 0.9],
        intensity: 3.0,
        range: 0.0,
        inner_deg: 0.0,
        outer_deg: 0.0,
        shadows: true,
    }
}

/// The lattice's side (spheres per edge) and its centre.
fn lattice(count: usize) -> (usize, Vec3) {
    let side = (count as f64).cbrt().ceil().max(1.0) as usize;
    (side, Vec3::new(0.0, 1.0 + side as f32 * 0.8, 0.0))
}

fn bench_frame(count: usize, kind: &str) -> RenderFrame {
    let mut instances = vec![instance(
        1,
        [0.0; 3],
        [120.0, 1.0, 120.0],
        look("plane", [0.5, 0.5, 0.48, 1.0]),
    )];
    for i in 0..12 {
        let hue = i as f32 * 0.7;
        instances.push(instance(
            2 + i as u64,
            [-16.5 + 3.0 * i as f32, 1.5, -16.0],
            [2.6; 3],
            look(
                "cube",
                [
                    0.5 + 0.4 * hue.sin(),
                    0.5 + 0.4 * (hue + 2.1).sin(),
                    0.5 + 0.4 * (hue + 4.2).sin(),
                    1.0,
                ],
            ),
        ));
    }
    let (side, centre) = lattice(count);
    let half = (side as f32 - 1.0) * 0.5;
    for k in 0..count {
        let (x, y, z) = (k % side, (k / side) % side, k / (side * side));
        let p = centre + 1.6 * Vec3::new(x as f32 - half, y as f32 - half, z as f32 - half);
        let hue = k as f32 * 0.37;
        let rgb = [
            0.55 + 0.4 * hue.sin(),
            0.55 + 0.4 * (hue + 2.1).sin(),
            0.55 + 0.4 * (hue + 4.2).sin(),
        ];
        instances.push(instance(
            100 + k as u64,
            p.to_array(),
            [1.0; 3],
            sphere_look(kind, rgb),
        ));
    }
    RenderFrame {
        tick: 1,
        dt_s: DT,
        reset: true,
        instances,
        lights: Some(vec![sun()]),
        cameras: Some(vec![]),
        environment: Some(environment()),
        ..RenderFrame::default()
    }
}

fn summary(v: &mut [f64]) -> serde_json::Value {
    v.sort_by(f64::total_cmp);
    let n = v.len().max(1);
    let round = |x: f64| (x * 1000.0).round() / 1000.0;
    let at = |p: usize| {
        round(
            v.get((v.len() * p / 100).min(n - 1))
                .copied()
                .unwrap_or(0.0),
        )
    };
    serde_json::json!({
        "mean": round(v.iter().sum::<f64>() / n as f64),
        "p50": at(50),
        "p95": at(95),
        "max": round(v.last().copied().unwrap_or(0.0)),
    })
}

fn target(gpu: &Gpu, (w, h): (u32, u32)) -> wgpu::TextureView {
    gpu.device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("transparency bench"),
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
        })
        .create_view(&Default::default())
}

fn bench() {
    let count: usize = arg("--count").and_then(|s| s.parse().ok()).unwrap_or(1000);
    let kind = arg("--kind").unwrap_or_else(|| "blend".into());
    let size = size((1600, 900));
    let frames: u32 = arg("--frames").and_then(|s| s.parse().ok()).unwrap_or(300);
    let warm: u32 = arg("--warm").and_then(|s| s.parse().ok()).unwrap_or(60);
    let gpu = Gpu::headless(BackendChoice::from_env()).expect("gpu");
    let init = std::time::Instant::now();
    let mut r = Renderer::new(&gpu, wgpu::TextureFormat::Rgba8UnormSrgb, size.0, size.1);
    let init_ms = init.elapsed().as_secs_f64() * 1000.0;
    r.set_prepass(
        arg("--prepass")
            .and_then(|s| PrepassMode::parse(&s))
            .unwrap_or(PrepassMode::Off),
    );
    r.set_occlusion(
        arg("--occlusion")
            .and_then(|s| OcclusionMode::parse(&s))
            .unwrap_or(OcclusionMode::Off),
    );
    r.add_model(DOUBLE, &double_model());
    r.apply(bench_frame(count, &kind), 0.0);
    let (side, centre) = lattice(count);
    let distance = side as f32 * 1.6 * 1.4 + 8.0;
    let orbit = flag("--orbit");
    let view = target(&gpu, size);
    if let Some(s) = arg("--settle").and_then(|s| s.parse::<f64>().ok()) {
        std::thread::sleep(std::time::Duration::from_secs_f64(s.max(0.0)));
    }
    let (mut wall, mut cpu, mut gpu_ms, mut sort) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    // The first frame's encoding (pipelines or buffers made on first use would show here).
    let mut first_cpu_ms = 0.0;
    let mut passes: Vec<(&'static str, f64, u32)> = Vec::new();
    for i in 0..warm + frames {
        let angle = if orbit { i as f32 * 0.01 } else { 0.35 };
        let eye = centre + distance * Vec3::new(angle.sin(), 0.25, angle.cos());
        r.set_camera_override(Some(CameraState::look_at(eye, centre)));
        let t = std::time::Instant::now();
        let stats = r.render(&view, f64::from(i) * DT);
        let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
        if i == 0 {
            first_cpu_ms = f64::from(stats.cpu_ms);
        }
        if i < warm {
            continue;
        }
        wall.push(t.elapsed().as_secs_f64() * 1000.0);
        cpu.push(f64::from(stats.cpu_ms));
        sort.push(f64::from(stats.transparent_ms));
        if !stats.passes.is_empty() {
            gpu_ms.push(f64::from(stats.gpu_ms));
        }
        for (l, ms) in &stats.passes {
            match passes.iter_mut().find(|(k, _, _)| k == l) {
                Some(e) => {
                    e.1 += f64::from(*ms);
                    e.2 += 1;
                }
                None => passes.push((l, f64::from(*ms), 1)),
            }
        }
    }
    let report = serde_json::json!({
        "bench": "transparency",
        "kind": kind,
        "count": count,
        "orbit": orbit,
        "backend": gpu.backend_name(),
        "adapter": gpu.info.name,
        "driver": format!("{} {}", gpu.info.driver, gpu.info.driver_info),
        "size": [size.0, size.1],
        "antialiasing": r.antialiasing().name(),
        "gtao": r.gtao().name(),
        "prepass": r.prepass().name(),
        "occlusion": r.occlusion().name(),
        "frames": frames,
        "init_ms": (init_ms * 1000.0).round() / 1000.0,
        "first_cpu_encode_ms": (first_cpu_ms * 1000.0).round() / 1000.0,
        "transparent_instances": r.last.transparent,
        "transparent_draws": r.last.transparent_draws,
        "draw_calls": r.last.draw_calls,
        "submit_to_idle_ms": summary(&mut wall),
        "cpu_encode_ms": summary(&mut cpu),
        "transparent_ms": summary(&mut sort),
        "gpu_ms": summary(&mut gpu_ms),
        "passes_ms": passes
            .iter()
            .map(|(l, s, n)| ((*l).to_owned(), serde_json::json!((s / f64::from(*n) * 1000.0).round() / 1000.0)))
            .collect::<serde_json::Map<_, _>>(),
    });
    println!("{report}");
}

/// The ghost scene at tick `k`: black and white stripes (emissive cubes) 6 m behind a sphere of
/// `kind`, which moves 0.1 m a tick across them (`object`) or stands (`pan`).
fn ghost_frame(k: u64, kind: &str, moving: bool) -> RenderFrame {
    let mut instances = Vec::new();
    for i in 0..24 {
        let white = i % 2 == 0;
        instances.push(InstanceUpdate {
            look: Some(Look {
                emissive: if white { [1.5; 3] } else { [0.02; 3] },
                ..look("cube", [0.0, 0.0, 0.0, 1.0])
            }),
            ..instance(
                1 + i as u64,
                [-11.5 + i as f32, 0.0, -6.0],
                [1.0, 12.0, 0.5],
                look("cube", [0.0; 4]),
            )
        });
    }
    let x = if moving { -3.0 + 0.1 * k as f32 } else { 0.0 };
    instances.push(InstanceUpdate {
        look: Some(Look {
            emissive: if kind == "glass" {
                [0.0; 3]
            } else {
                [0.6, 0.15, 0.1]
            },
            ..sphere_look(kind, [0.0, 0.0, 0.0])
        }),
        ..instance(100, [x, 0.0, 0.0], [2.0; 3], look("sphere", [0.0; 4]))
    });
    RenderFrame {
        tick: k,
        dt_s: DT,
        reset: k == 1,
        instances,
        lights: Some(vec![]),
        cameras: Some(vec![CameraView {
            id: 1000,
            position: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            fov_deg: 50.0,
            near: 0.1,
            far: 1000.0,
            active: true,
            exposure_ev: 0.0,
        }]),
        environment: Some(EnvironmentView {
            sky: 1,
            sky_color: [0.0; 3],
            ambient: 0.3,
            ..environment()
        }),
        ..RenderFrame::default()
    }
}

fn luminance(img: &[f32], i: usize) -> f32 {
    0.2126 * img[4 * i] + 0.7152 * img[4 * i + 1] + 0.0722 * img[4 * i + 2]
}

fn ghost() {
    let kind = arg("--kind").unwrap_or_else(|| "blend".into());
    let moving = arg("--motion").is_none_or(|m| m != "pan");
    let size = size((960, 540));
    let frames: u64 = arg("--frames").and_then(|s| s.parse().ok()).unwrap_or(48);
    let gpu = Gpu::headless(BackendChoice::from_env()).expect("gpu");
    let renderer = |aa: Antialiasing| {
        let mut r = Renderer::new(&gpu, wgpu::TextureFormat::Rgba8UnormSrgb, size.0, size.1);
        r.set_antialiasing(aa);
        r.set_gtao(Gtao::Off);
        r.add_model(DOUBLE, &double_model());
        r
    };
    let camera = |k: u64| {
        // Panning: 0.1 m a tick to the right, looking straight ahead.
        let x = if moving { 0.0 } else { -3.0 + 0.1 * k as f32 };
        CameraState::look_at(Vec3::new(x, 0.0, 9.0), Vec3::new(x, 0.0, 0.0))
    };
    let mut renderers = [renderer(Antialiasing::Taa), renderer(Antialiasing::Msaa)];
    // The sphere's pixels at each of the last nine frames (from the 4x MSAA renderer's id pass).
    let mut masks: Vec<Vec<bool>> = Vec::new();
    let mut images = Vec::new();
    for k in 1..=frames {
        let t = k as f64 * DT;
        images.clear();
        for (j, r) in renderers.iter_mut().enumerate() {
            r.apply(ghost_frame(k, &kind, moving), t);
            r.set_camera_override(Some(camera(k)));
            let mask = j == 1 && k + 9 > frames;
            if mask {
                r.request_id_image();
            }
            // At the tick's own pose (alpha 1).
            images.push(r.capture_hdr(t + DT).2);
            if mask {
                let mut ids = None;
                for _ in 0..30 {
                    if r.take_visible().is_some() {
                        ids = r.take_id_image();
                        break;
                    }
                    let _ = r.capture_hdr(t + DT);
                }
                let ids = ids.expect("the id pass answers");
                masks.push(ids.iter().map(|&e| e == 100).collect());
            }
        }
    }
    // The reference: the last moment held still, the mean of 64 frames jittered over the pixel.
    let mut reference = renderer(Antialiasing::Taa);
    reference.taa_mut().mode = amoris_render::taa::TaaMode::Jittered;
    reference.apply(ghost_frame(1, &kind, moving), 0.0);
    let t = frames as f64 * DT;
    reference.apply(ghost_frame(frames, &kind, moving), t);
    reference.set_camera_override(Some(camera(frames)));
    for _ in 0..16 {
        let _ = reference.capture_hdr(t + DT);
    }
    let mut sum = vec![0.0f64; (size.0 * size.1 * 4) as usize];
    const REFERENCE_FRAMES: u32 = 64;
    for _ in 0..REFERENCE_FRAMES {
        for (s, v) in sum.iter_mut().zip(reference.capture_hdr(t + DT).2) {
            *s += f64::from(v);
        }
    }
    let reference: Vec<f32> = sum
        .iter()
        .map(|s| (s / f64::from(REFERENCE_FRAMES)) as f32)
        .collect();
    let now = masks.last().expect("masks").clone();
    let trail: Vec<bool> = (0..now.len())
        .map(|i| !now[i] && masks[..masks.len() - 1].iter().any(|m| m[i]))
        .collect();
    let everywhere = vec![true; now.len()];
    let error = |img: &[f32], mask: &[bool]| {
        let (mut sum, mut n) = (0.0f64, 0u32);
        for (i, &on) in mask.iter().enumerate() {
            if on {
                sum += f64::from((luminance(img, i) - luminance(&reference, i)).abs());
                n += 1;
            }
        }
        ((sum / f64::from(n.max(1))) * 1e5).round() / 1e5
    };
    let regions = |img: &[f32]| {
        serde_json::json!({
            "sphere": error(img, &now),
            "trail": error(img, &trail),
            "image": error(img, &everywhere),
        })
    };
    let report = serde_json::json!({
        "bench": "transparency ghost",
        "kind": kind,
        "motion": if moving { "object" } else { "pan" },
        "backend": gpu.backend_name(),
        "adapter": gpu.info.name,
        "size": [size.0, size.1],
        "frames": frames,
        "sphere_pixels": now.iter().filter(|&&b| b).count(),
        "trail_pixels": trail.iter().filter(|&&b| b).count(),
        "reference_frames": REFERENCE_FRAMES,
        // Mean absolute difference from the reference in linear luminance.
        "taa": regions(&images[0]),
        "msaa": regions(&images[1]),
    });
    if let Some(dir) = arg("--save") {
        // TAA, MSAA and the reference side by side, Reinhard-mapped sRGB.
        let (w, h) = size;
        let mut strip = vec![0u8; (w * 3 * h * 4) as usize];
        for (k, img) in [&images[0], &images[1], &reference].into_iter().enumerate() {
            for y in 0..h {
                for x in 0..w {
                    let i = ((y * w + x) * 4) as usize;
                    let o = ((y * w * 3 + k as u32 * w + x) * 4) as usize;
                    for c in 0..3 {
                        let v = img[i + c].max(0.0);
                        let v = v / (1.0 + v);
                        let v = if v <= 0.003_130_8 {
                            12.92 * v
                        } else {
                            1.055 * v.powf(1.0 / 2.4) - 0.055
                        };
                        strip[o + c] = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
                    }
                    strip[o + 3] = 255;
                }
            }
        }
        let motion = if moving { "object" } else { "pan" };
        let path = std::path::Path::new(&dir).join(format!("ghost-{kind}-{motion}.png"));
        image::save_buffer(&path, &strip, w * 3, h, image::ColorType::Rgba8).expect("save");
    }
    println!("{report}");
}

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("bench") => bench(),
        Some("ghost") => ghost(),
        _ => {
            eprintln!("usage: transparency bench|ghost [options] (see the source's documentation)")
        }
    }
}
