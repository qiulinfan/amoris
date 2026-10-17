//! Culling shadow casters by each cascade's extruded slice leaves the image alone
//! (docs/spec/occlusion.md 11). Each scene is drawn by two renderers on the same device through
//! the same frames at the same moments, one with `ShadowCull::Off` and one with `On`; every frame
//! must be identical to the bit, the camera's draws unchanged, and no cascade may draw more with
//! the slices than without:
//!
//! - the mixed scene (every pipeline variant, alpha-masked and double-sided casters) with three
//!   skinned characters (their casters' grown spheres), its camera circling it, low and high;
//! - a small levels-of-detail field with the camera walking into it at head height (levels on, and
//!   off), where the slices must cull something in every cascade;
//! - the many_cubes sphere of shadow-casting cubes, its camera turning.
//!
//! Runs on the device `AMORIS_BACKEND`, `AMORIS_ADAPTER` and `AMORIS_GPU_MINIMAL` describe, with
//! the adapter's default anti-aliasing (TAA's history must accumulate the same images). Skips
//! without a GPU.

use amoris_assets::frame::{AnimView, InstanceUpdate, Look, Pose, RenderFrame};
use amoris_assets::mesh::ModelAsset;
use amoris_render::{
    BackendChoice, CameraState, DrawCounts, Gpu, LodMode, OcclusionMode, PrepassMode, Renderer,
    ShadowCull, demo,
};
use glam::Vec3;

const W: u32 = 480;
const H: u32 = 270;
const HERO: &str = "models/hero.glb";

fn hero() -> ModelAsset {
    let glb = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../samples/anim/models/hero.glb"
    ))
    .expect("samples/anim/models/hero.glb");
    amoris_assets::import::import_glb_bytes(&glb).expect("hero.glb imports")
}

/// `demo::mixed(6)` with three skinned characters among the primitives, as tests/prepass.rs.
fn mixed() -> RenderFrame {
    let mut frame = demo::mixed(6);
    for i in 0..3u64 {
        frame.instances.push(InstanceUpdate {
            id: 1000 + i,
            pose: Some(Pose {
                position: [-3.0 + 3.0 * i as f32, 0.0, 9.5],
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

struct Scene {
    name: &'static str,
    frame: RenderFrame,
    models: Vec<(&'static str, ModelAsset)>,
    cameras: Vec<CameraState>,
    lod: LodMode,
}

fn run(gpu: &Gpu, s: &Scene, cull: ShadowCull) -> Vec<(Vec<u8>, DrawCounts)> {
    let mut r = Renderer::new(gpu, wgpu::TextureFormat::Rgba8UnormSrgb, W, H);
    r.set_occlusion(OcclusionMode::Off);
    r.set_prepass(PrepassMode::Off);
    r.set_lod(s.lod);
    r.set_shadow_cull(cull);
    for (path, model) in &s.models {
        r.add_model(path, model);
    }
    r.apply(s.frame.clone(), 0.0);
    s.cameras
        .iter()
        .enumerate()
        .map(|(i, cam)| {
            r.set_camera_override(Some(*cam));
            let (_, _, rgba) = r.capture_rgba(1.0 / 120.0 + i as f64 / 60.0);
            (rgba, r.draw_counts())
        })
        .collect()
}

fn scenes() -> Vec<Scene> {
    let orbit = (0..8)
        .map(|i| {
            let a = i as f32 / 8.0 * std::f32::consts::TAU;
            let eye = Vec3::new(a.sin() * 18.0, 3.0 + 9.0 * (i % 2) as f32, a.cos() * 18.0);
            CameraState::look_at(eye, Vec3::new(0.0, 0.5, 0.0))
        })
        .collect();
    let (n, spacing) = (16, 6.0);
    let walk: Vec<CameraState> = (0..6)
        .map(|i| demo::lod_field_camera(n, spacing, i as f32 / 5.0))
        .collect();
    let field = |name, lod| Scene {
        name,
        frame: demo::lod_field(n, spacing),
        models: vec![(demo::LOD_MODEL, demo::lod_model(4))],
        cameras: walk.clone(),
        lod,
    };
    vec![
        Scene {
            name: "mixed orbit",
            frame: mixed(),
            models: vec![(demo::MIXED_MODEL, demo::mixed_model()), (HERO, hero())],
            cameras: orbit,
            lod: LodMode::On,
        },
        field("field walk, levels on", LodMode::On),
        field("field walk, levels off", LodMode::Off),
        Scene {
            name: "cubes sphere",
            frame: demo::many_cubes(20_000, false, true),
            models: Vec::new(),
            cameras: (0..4).map(|i| demo::cubes_camera(i * 90, false)).collect(),
            lod: LodMode::On,
        },
    ]
}

#[test]
fn slices_cull_casters_and_leave_the_image_alone() {
    let Ok(gpu) = Gpu::headless(BackendChoice::from_env()) else {
        eprintln!("no GPU: skipped");
        return;
    };
    for s in scenes() {
        let off = run(&gpu, &s, ShadowCull::Off);
        let on = run(&gpu, &s, ShadowCull::On);
        let mut culled = [0u64; 4];
        for (i, ((a, ca), (b, cb))) in off.iter().zip(&on).enumerate() {
            let differing = a
                .as_chunks::<4>()
                .0
                .iter()
                .zip(b.as_chunks::<4>().0)
                .filter(|(p, q)| p != q)
                .count();
            assert_eq!(
                differing, 0,
                "{}, frame {i}: {differing} pixels differ",
                s.name
            );
            assert_eq!(
                ca.camera, cb.camera,
                "{}, frame {i}: the camera's draws",
                s.name
            );
            for (c, (x, y)) in ca.cascades.iter().zip(&cb.cascades).enumerate() {
                assert!(
                    y.instances() <= x.instances() && y.triangles() <= x.triangles(),
                    "{}, frame {i}, cascade {c}: {:?} on against {:?} off",
                    s.name,
                    y,
                    x
                );
                culled[c] += x.instances() - y.instances();
            }
        }
        let shadow =
            |v: &[(Vec<u8>, DrawCounts)]| v.iter().map(|(_, c)| c.shadow_triangles()).sum::<u64>();
        eprintln!(
            "{}: {} frames identical; cascade instances culled per cascade {culled:?}; shadow \
             triangles {} off, {} on",
            s.name,
            off.len(),
            shadow(&off),
            shadow(&on)
        );
        if s.name.starts_with("field") {
            assert!(culled.iter().all(|&n| n > 0), "{}: {culled:?}", s.name);
        }
    }
}
