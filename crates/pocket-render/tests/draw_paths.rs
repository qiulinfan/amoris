//! The three draw paths of batches.rs draw the same frame. The mixed demo scene (every pipeline
//! variant, the camera and four shadow cascades) plus skinned characters is rendered headless on
//! a device with every feature the adapter offers (natively: one multi-draw per view and variant),
//! on one drawing batch by batch with the base in `first_instance` (the browser with
//! `indirect-first-instance`), and on one without `indirect-first-instance` (WebGPU's baseline),
//! whose wgpu-core indirect-call validation turns a draw with a nonzero `first_instance` into a
//! no-op as a browser does. The entity-id pass's coverage and the pixels must agree. Skips when
//! there is no GPU.

use pocket_assets::frame::{AnimView, InstanceUpdate, Look, Pose, RenderFrame};
use pocket_render::gpu::Minimal;
use pocket_render::{BackendChoice, Gpu, Renderer, demo};

const N: u32 = 6;
const HERO: &str = "models/hero.glb";

fn scene() -> RenderFrame {
    let mut frame = demo::mixed(N);
    // Skinned meshes: each character's skinned parts are meshes of their own, drawn in the
    // same batches as everything else.
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

struct Shot {
    rgba: Vec<u8>,
    visible: Vec<(u64, f32)>,
    first_instance: bool,
    path: &'static str,
}

fn shoot(gpu: &Gpu) -> Shot {
    let hero = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../samples/anim/models/hero.glb"
    ))
    .expect("samples/anim/models/hero.glb");
    let hero = pocket_assets::import::import_glb_bytes(&hero).expect("hero.glb imports");
    let mut r = Renderer::new(gpu, wgpu::TextureFormat::Rgba8UnormSrgb, 480, 270);
    r.add_model(demo::MIXED_MODEL, &demo::mixed_model());
    r.add_model(HERO, &hero);
    r.apply(scene(), 0.0);
    r.set_camera_override(Some(demo::mixed_camera(N)));
    let t = 1.0 / 120.0;
    r.request_visible();
    let mut visible = None;
    for _ in 0..30 {
        let _ = r.capture_rgba(t);
        if let Some(v) = r.take_visible() {
            visible = Some(v);
            break;
        }
    }
    let (_, _, rgba) = r.capture_rgba(t);
    Shot {
        rgba,
        visible: visible.expect("the id pass answers"),
        first_instance: gpu.caps.indirect_first_instance,
        path: r.draw_path(),
    }
}

/// The two shots show the same entities with the same coverage and nearly the same pixels.
fn agree(a: &Shot, b: &Shot) {
    eprintln!(
        "{} vs {}: {} and {} entities visible",
        a.path,
        b.path,
        a.visible.len(),
        b.visible.len()
    );
    let share = |v: &[(u64, f32)], e: u64| v.iter().find(|x| x.0 == e).map_or(0.0, |x| x.1);
    for (e, s) in &a.visible {
        let d = (s - share(&b.visible, *e)).abs();
        assert!(
            d < 0.002,
            "{} vs {}: entity {e} covers {s} of the view, {d} off",
            a.path,
            b.path
        );
    }
    assert_eq!(a.visible.len(), b.visible.len(), "{} vs {}", a.path, b.path);
    let differing = a
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.rgba.as_chunks::<4>().0)
        .filter(|(p, q)| (0..3).any(|i| p[i].abs_diff(q[i]) > 8))
        .count();
    let share = differing as f64 / (a.rgba.len() / 4) as f64;
    assert!(
        share < 0.002,
        "{} vs {}: {differing} pixels differ ({share:.4})",
        a.path,
        b.path
    );
}

#[test]
fn every_draw_path_draws_the_same_frame() {
    let choice = BackendChoice::from_env();
    let Ok(full) = Gpu::headless_with(choice, Minimal::default()) else {
        eprintln!("no GPU: skipped");
        return;
    };
    let device = |m: Minimal| Gpu::headless_with(choice, m).expect("another device");
    // Batch by batch with the base in first_instance (the browser with the feature).
    let per_batch = device(Minimal {
        multi_draw: true,
        ..Minimal::default()
    });
    // WebGPU's baseline.
    let baseline = device(Minimal {
        first_instance: true,
        ..Minimal::default()
    });
    let a = shoot(&full);
    if !a.first_instance {
        eprintln!("the adapter has no indirect-first-instance: every shot takes the baseline path");
    }
    // Most of the scene is on screen: the ground, the cells and the skinned characters.
    assert!(a.visible.len() >= 30, "{:?}", a.visible);
    for e in [1, 1000, 1001, 1002] {
        assert!(a.visible.iter().any(|v| v.0 == e), "{e}: {:?}", a.visible);
    }
    let c = shoot(&baseline);
    assert!(!c.first_instance && c.path == "baseline");
    agree(&a, &c);
    agree(&a, &shoot(&per_batch));
}
