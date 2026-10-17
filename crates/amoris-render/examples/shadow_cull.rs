//! Shadow caster culling by each cascade's extruded slice (docs/spec/occlusion.md 11,
//! docs/bench/shadows.md): the same frames with `ShadowCull::Off` and `On`, compared to the bit,
//! with what each cascade drew; and the cost of both, in interleaved blocks of frames.
//!
//! ```sh
//! cargo run --release -p amoris-render --example shadow_cull -- compare [--size 1280x720]
//!     [--out DIR] [--count 200000] [--occlusion off|on|auto] [--prepass off|on|auto] [SCENE ...]
//! cargo run --release -p amoris-render --example shadow_cull -- bench SCENE [--size 1280x720]
//!     [--count 200000] [--rounds 4] [--block 40] [--skip 6] [--settle 5]
//! ```
//!
//! Scenes: `mixed` (demo::mixed(8): every pipeline variant), `mixed-orbit` (the camera circling it,
//! 24 frames, three skinned characters from samples/anim among the primitives), `cubes-sphere`
//! (many_cubes' sphere of `--count` shadow-casting cubes, its camera turning, 8 frames),
//! `cubes-dense` (the dense layout), `lod-0` and `lod-0.5` (demo::lod_field at `t` 0 and 0.5,
//! levels of detail on), `lod-off-0` and `lod-off-0.5` (levels off), `lod-fly` and `lod-off-fly`
//! (the field's camera walking from `t` 0 to 1 over 24 frames). Defaults: every scene.
//!
//! `compare` draws each scene with two renderers on one device (the environment's backend, adapter
//! and `AMORIS_GPU_MINIMAL`), slice culling off and on, through the same frames at the same
//! moments; occlusion culling and the depth prepass are off unless `--occlusion` and `--prepass`
//! say otherwise (the prepass's auto mode decides by GPU timings, which differ between runs).
//! Prints one JSON line per scene: frames compared, frames identical to the bit, pixels that
//! differ, and per cascade the instances and triangles the last frame drew off and on (read back
//! from the indirect arguments, `Renderer::draw_counts`), with their sums over every frame. Writes
//! `<scene>-off.png`, `<scene>-on.png` (the last frame) and, if any frame differs,
//! `<scene>-diff.png` (the first that differs, amplified) and `shadow_cull.json` to `--out`.
//!
//! `bench` draws one scene with one renderer offscreen, switching slice culling every `--block`
//! measured frames (`--skip` frames after each switch are drawn but not kept, so pass timings read
//! back late are never counted under the other mode), `--rounds` rounds, off first in odd rounds
//! and on first in even ones. Each frame is submitted and waited for. Prints one JSON object with,
//! per mode and round, the frame's GPU time, the four cascades' summed `shadows` passes, the `cull`
//! pass, wall time (submit to idle) and CPU encoding, and the draw counts.

use std::path::{Path, PathBuf};
use std::time::Instant;

use amoris_assets::frame::{AnimView, InstanceUpdate, Look, Pose, RenderFrame};
use amoris_assets::mesh::ModelAsset;
use amoris_render::lod::SetCounts;
use amoris_render::{
    BackendChoice, CameraState, DrawCounts, Gpu, LodMode, OcclusionMode, PrepassMode, Renderer,
    ShadowCull, demo,
};
use glam::Vec3;
use serde_json::{Value, json};

fn arg(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

fn number<T: std::str::FromStr>(name: &str, default: T) -> T {
    arg(name).and_then(|s| s.parse().ok()).unwrap_or(default)
}

const SCENES: [&str; 10] = [
    "mixed",
    "mixed-orbit",
    "cubes-sphere",
    "cubes-dense",
    "lod-0",
    "lod-0.5",
    "lod-off-0",
    "lod-off-0.5",
    "lod-fly",
    "lod-off-fly",
];
const FIELD: (u32, f32) = (48, 6.0);
const HERO: &str = "models/hero.glb";

fn hero() -> Result<ModelAsset, String> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../samples/anim/models/hero.glb"
    );
    let glb = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    amoris_assets::import::import_glb_bytes(&glb).map_err(|e| format!("{path}: {e:?}"))
}

/// `demo::mixed(8)` with three animated skinned characters in front of the grid.
fn mixed_skinned() -> RenderFrame {
    let mut frame = demo::mixed(8);
    for i in 0..3u64 {
        frame.instances.push(InstanceUpdate {
            id: 1000 + i,
            pose: Some(Pose {
                position: [-3.0 + 3.0 * i as f32, 0.0, 12.5],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.6; 3],
            }),
            look: Some(Look {
                mesh: HERO.into(),
                material: String::new(),
                color: [1.0; 4],
                metallic: 0.0,
                roughness: 0.6,
                transmission: None,
                ior: None,
                emissive: [0.0; 3],
                cast_shadows: true,
                visible: true,
            }),
            anim: Some(AnimView {
                clip: "idle".into(),
                time: 0.3 * i as f32,
                rate: 1.0,
                looped: true,
            }),
        });
    }
    frame
}

/// A scene: its first frame, the models it needs, its camera per frame, how many frames a
/// comparison draws, and whether levels of detail are on.
struct Scene {
    frame: RenderFrame,
    models: Vec<(&'static str, ModelAsset)>,
    camera: Box<dyn Fn(u32) -> CameraState>,
    frames: u32,
    lod: LodMode,
}

fn scene(name: &str, count: usize) -> Result<Scene, String> {
    let lod_scene = |t: Option<f32>, lod: LodMode| {
        let (n, spacing) = FIELD;
        Scene {
            frame: demo::lod_field(n, spacing),
            models: vec![(demo::LOD_MODEL, demo::lod_model(6))],
            camera: match t {
                Some(t) => Box::new(move |_| demo::lod_field_camera(n, spacing, t)),
                None => Box::new(move |i| demo::lod_field_camera(n, spacing, i as f32 / 23.0)),
            },
            frames: if t.is_some() { 3 } else { 24 },
            lod,
        }
    };
    Ok(match name {
        "mixed" => Scene {
            frame: demo::mixed(8),
            models: vec![(demo::MIXED_MODEL, demo::mixed_model())],
            camera: Box::new(|_| demo::mixed_camera(8)),
            frames: 3,
            lod: LodMode::On,
        },
        "mixed-orbit" => Scene {
            frame: mixed_skinned(),
            models: vec![(demo::MIXED_MODEL, demo::mixed_model()), (HERO, hero()?)],
            // A full turn around the grid in 24 frames, low and high.
            camera: Box::new(|i| {
                let a = i as f32 / 24.0 * std::f32::consts::TAU;
                let d = 8.0 * 3.0;
                let eye = Vec3::new(a.sin() * d, 4.0 + 10.0 * (i % 2) as f32, a.cos() * d);
                CameraState::look_at(eye, Vec3::new(0.0, 0.5, 0.0))
            }),
            frames: 24,
            lod: LodMode::On,
        },
        "cubes-sphere" => Scene {
            frame: demo::many_cubes(count, false, true),
            models: Vec::new(),
            camera: Box::new(|i| demo::cubes_camera(i * 60, false)),
            frames: 8,
            lod: LodMode::On,
        },
        "cubes-dense" => Scene {
            frame: demo::many_cubes(count, true, true),
            models: Vec::new(),
            camera: Box::new(|i| demo::cubes_camera(i, true)),
            frames: 3,
            lod: LodMode::On,
        },
        "lod-0" => lod_scene(Some(0.0), LodMode::On),
        "lod-0.5" => lod_scene(Some(0.5), LodMode::On),
        "lod-off-0" => lod_scene(Some(0.0), LodMode::Off),
        "lod-off-0.5" => lod_scene(Some(0.5), LodMode::Off),
        "lod-fly" => lod_scene(None, LodMode::On),
        "lod-off-fly" => lod_scene(None, LodMode::Off),
        other => return Err(format!("unknown scene {other:?} (known: {SCENES:?})")),
    })
}

fn renderer(gpu: &Gpu, size: (u32, u32), s: &Scene, cull: ShadowCull) -> Renderer {
    let mut r = Renderer::new(gpu, wgpu::TextureFormat::Rgba8UnormSrgb, size.0, size.1);
    r.set_occlusion(
        arg("--occlusion")
            .and_then(|m| OcclusionMode::parse(&m))
            .unwrap_or(OcclusionMode::Off),
    );
    r.set_prepass(
        arg("--prepass")
            .and_then(|m| PrepassMode::parse(&m))
            .unwrap_or(PrepassMode::Off),
    );
    r.set_lod(s.lod);
    r.set_shadow_cull(cull);
    for (path, model) in &s.models {
        r.add_model(path, model);
    }
    r.apply(s.frame.clone(), 0.0);
    r
}

fn set_json(s: &SetCounts) -> Value {
    json!({"instances": s.instances(), "triangles": s.triangles()})
}

fn cascades_json(c: &DrawCounts) -> Value {
    json!(c.cascades.iter().map(set_json).collect::<Vec<_>>())
}

fn add(sum: &mut [[u64; 2]; 4], c: &DrawCounts) {
    for (s, set) in sum.iter_mut().zip(&c.cascades) {
        s[0] += set.instances();
        s[1] += set.triangles();
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("compare") => compare(&args[1..]),
        Some("bench") => bench(&args[1..]),
        _ => Err("usage: shadow_cull compare [SCENE ...] | bench SCENE (see the source)".into()),
    };
    if let Err(e) = result {
        eprintln!("shadow_cull: {e}");
        std::process::exit(1);
    }
}

fn size() -> (u32, u32) {
    arg("--size")
        .and_then(|s| {
            s.split_once('x')
                .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
        })
        .unwrap_or((1280, 720))
}

/// The positional arguments (scene names), skipping options and their values.
fn positional(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut skip = false;
    for a in args {
        if skip {
            skip = false;
        } else if a.starts_with("--") {
            skip = true;
        } else {
            out.push(a.clone());
        }
    }
    out
}

/// One renderer's run through a scene: every frame's pixels and draw counts.
fn run(
    gpu: &Gpu,
    size: (u32, u32),
    s: &Scene,
    cull: ShadowCull,
) -> (Vec<Vec<u8>>, Vec<DrawCounts>, &'static str) {
    let mut r = renderer(gpu, size, s, cull);
    let (mut images, mut counts) = (Vec::new(), Vec::new());
    for i in 0..s.frames {
        r.set_camera_override(Some((s.camera)(i)));
        // Fixed moments, the same for both renderers.
        let (_, _, rgba) = r.capture_rgba(1.0 / 120.0 + f64::from(i) / 60.0);
        images.push(rgba);
        counts.push(r.draw_counts());
    }
    (images, counts, r.draw_path())
}

fn compare(args: &[String]) -> Result<(), String> {
    let out = PathBuf::from(arg("--out").unwrap_or_else(|| "out/shadowcull/compare".into()));
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let size = size();
    let count: usize = number("--count", 200_000);
    let mut names = positional(args);
    if names.is_empty() {
        names = SCENES.iter().map(|s| (*s).to_owned()).collect();
    }
    let gpu = Gpu::headless(BackendChoice::from_env()).map_err(|e| e.to_string())?;
    let mut report = Vec::new();
    for name in &names {
        let s = scene(name, count)?;
        let t = Instant::now();
        let (off_px, off_n, draw_path) = run(&gpu, size, &s, ShadowCull::Off);
        let (on_px, on_n, _) = run(&gpu, size, &s, ShadowCull::On);
        let mut identical = 0;
        let mut differing = 0u64;
        let mut max = 0u8;
        let mut first_diff = None;
        for (i, (a, b)) in off_px.iter().zip(&on_px).enumerate() {
            if a == b {
                identical += 1;
                continue;
            }
            for (pa, pb) in a.as_chunks::<4>().0.iter().zip(b.as_chunks::<4>().0) {
                let d = (0..4).map(|k| pa[k].abs_diff(pb[k])).max().unwrap_or(0);
                differing += u64::from(d > 0);
                max = max.max(d);
            }
            first_diff.get_or_insert(i);
        }
        let file = name.replace('.', "_");
        let save = |suffix: &str, px: &[u8]| {
            image::save_buffer(
                out.join(format!("{file}-{suffix}.png")),
                px,
                size.0,
                size.1,
                image::ColorType::Rgba8,
            )
            .map_err(|e| e.to_string())
        };
        save("off", off_px.last().ok_or("no frames")?)?;
        save("on", on_px.last().ok_or("no frames")?)?;
        if let Some(i) = first_diff {
            let diff: Vec<u8> = off_px[i]
                .as_chunks::<4>()
                .0
                .iter()
                .zip(on_px[i].as_chunks::<4>().0)
                .flat_map(|(a, b)| {
                    let d = (0..3).map(|k| a[k].abs_diff(b[k])).max().unwrap_or(0);
                    let v = d.saturating_mul(16);
                    [v, v, v, 255]
                })
                .collect();
            save("diff", &diff)?;
        }
        let (mut sum_off, mut sum_on) = ([[0u64; 2]; 4], [[0u64; 2]; 4]);
        off_n.iter().for_each(|c| add(&mut sum_off, c));
        on_n.iter().for_each(|c| add(&mut sum_on, c));
        let (last_off, last_on) = (
            off_n.last().ok_or("no frames")?,
            on_n.last().ok_or("no frames")?,
        );
        // The camera's draws must not change at all.
        let camera_same = off_n
            .iter()
            .zip(&on_n)
            .all(|(a, b)| a.camera == b.camera && a.late == b.late);
        let line = json!({
            "scene": name,
            "backend": gpu.backend_name(),
            "adapter": gpu.info.name,
            "draw_path": draw_path,
            "size": [size.0, size.1],
            "frames": s.frames,
            "identical_frames": identical,
            "differing_pixels": differing,
            "max_abs_diff": max,
            "first_differing_frame": first_diff,
            "camera_draws_same": camera_same,
            "camera_triangles": last_on.camera_triangles(),
            "last_frame": {"off": cascades_json(last_off), "on": cascades_json(last_on)},
            "shadow_triangles_last": [last_off.shadow_triangles(), last_on.shadow_triangles()],
            "all_frames": {
                "off": sum_off.map(|[i, t]| json!({"instances": i, "triangles": t})),
                "on": sum_on.map(|[i, t]| json!({"instances": i, "triangles": t})),
            },
            "seconds": (t.elapsed().as_secs_f64() * 10.0).round() / 10.0,
        });
        println!("{line}");
        report.push(line);
    }
    std::fs::write(
        out.join("shadow_cull.json"),
        serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

fn stats(v: &[f64]) -> Value {
    let mut v = v.to_vec();
    v.sort_by(f64::total_cmp);
    let n = v.len().max(1);
    let round = |x: f64| (x * 1000.0).round() / 1000.0;
    let at = |p: usize| {
        round(
            v.get((v.len() * p / 100).min(v.len().saturating_sub(1)))
                .copied()
                .unwrap_or(0.0),
        )
    };
    json!({
        "n": v.len(),
        "p50": at(50),
        "mean": round(v.iter().sum::<f64>() / n as f64),
        "min": round(v.first().copied().unwrap_or(0.0)),
        "p95": at(95),
    })
}

#[derive(Default)]
struct Samples {
    gpu: Vec<f64>,
    shadows: Vec<f64>,
    cull: Vec<f64>,
    wall: Vec<f64>,
    cpu: Vec<f64>,
}

impl Samples {
    fn json(&self) -> Value {
        json!({
            "gpu_ms": stats(&self.gpu),
            "shadows_ms": stats(&self.shadows),
            "cull_ms": stats(&self.cull),
            "wall_ms": stats(&self.wall),
            "cpu_ms": stats(&self.cpu),
        })
    }
}

fn bench(args: &[String]) -> Result<(), String> {
    let names = positional(args);
    let name = names.first().ok_or("bench: which scene?")?;
    let size = size();
    let count: usize = number("--count", 200_000);
    let rounds: u32 = number("--rounds", 4);
    let block: u32 = number("--block", 40);
    let skip: u32 = number("--skip", 6).max(3);
    let s = scene(name, count)?;
    let gpu = Gpu::headless(BackendChoice::from_env()).map_err(|e| e.to_string())?;
    let mut r = renderer(&gpu, size, &s, ShadowCull::Off);
    let tex = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("bench target"),
        size: wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = tex.create_view(&Default::default());
    // One fixed camera and moment: only the mode changes between blocks.
    r.set_camera_override(Some((s.camera)(0)));
    let now = 1.0;
    // Warm up both modes (pipelines, first-use costs).
    for mode in [ShadowCull::Off, ShadowCull::On] {
        r.set_shadow_cull(mode);
        for _ in 0..10 {
            r.render(&view, now);
            let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
        }
    }
    if let Some(sec) = arg("--settle").and_then(|s| s.parse::<f64>().ok()) {
        std::thread::sleep(std::time::Duration::from_secs_f64(sec.max(0.0)));
    }
    let mut all = [Samples::default(), Samples::default()];
    let mut per_round = Vec::new();
    let mut counts: [Option<DrawCounts>; 2] = [None, None];
    for round in 0..rounds {
        let order = if round % 2 == 0 {
            [ShadowCull::Off, ShadowCull::On]
        } else {
            [ShadowCull::On, ShadowCull::Off]
        };
        let mut row = serde_json::Map::new();
        for mode in order {
            let m = usize::from(mode == ShadowCull::On);
            r.set_shadow_cull(mode);
            let mut block_samples = Samples::default();
            for i in 0..skip + block {
                let t = Instant::now();
                let st = r.render(&view, now);
                let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
                let wall = t.elapsed().as_secs_f64() * 1000.0;
                if i < skip || st.passes.is_empty() {
                    continue;
                }
                let pass = |label: &str| {
                    st.passes
                        .iter()
                        .filter(|(l, _)| *l == label)
                        .map(|(_, ms)| f64::from(*ms))
                        .sum::<f64>()
                };
                for smp in [&mut block_samples, &mut all[m]] {
                    smp.gpu.push(f64::from(st.gpu_ms));
                    smp.shadows.push(pass("shadows"));
                    smp.cull.push(pass("cull"));
                    smp.wall.push(wall);
                    smp.cpu.push(f64::from(st.cpu_ms));
                }
            }
            counts[m] = Some(r.draw_counts());
            row.insert(mode.name().into(), block_samples.json());
        }
        per_round.push(Value::Object(row));
    }
    let counts_json = |c: &Option<DrawCounts>| {
        c.as_ref().map(|c| {
            json!({
                "cascades": cascades_json(c),
                "shadow_triangles": c.shadow_triangles(),
                "camera_triangles": c.camera_triangles(),
            })
        })
    };
    let report = json!({
        "bench": "shadow_cull",
        "scene": name,
        "backend": gpu.backend_name(),
        "adapter": gpu.info.name,
        "driver": format!("{} {}", gpu.info.driver, gpu.info.driver_info),
        "size": [size.0, size.1],
        "antialiasing": r.antialiasing().name(),
        "gtao": r.gtao().name(),
        "draw_path": r.draw_path(),
        "occlusion": r.last.occlusion,
        "prepass": r.last.prepass,
        "lod": r.last.lod,
        "instances": r.last.instances,
        "rounds": rounds,
        "block": block,
        "skip": skip,
        "off": all[0].json(),
        "on": all[1].json(),
        "per_round": per_round,
        "counts": {"off": counts_json(&counts[0]), "on": counts_json(&counts[1])},
    });
    println!("{report}");
    if let Some(path) = arg("--json") {
        let p = Path::new(&path);
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(
            p,
            serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}
