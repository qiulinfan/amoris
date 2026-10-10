//! What changes after the first frames draws as it would in a renderer built for the changed
//! scene. The renderer keeps per-frame GPU work between frames where nothing changed (the skinning
//! pass's jobs and bind group, the display transform's parameters; docs/bench/dx12.md 10.4); these
//! checks change what that work depends on and compare the image with a fresh renderer's:
//!
//! - Skinned characters replaced and added: one removed and one added (the jobs change, the jobs
//!   buffer does not), then six more (the jobs, joint matrices and vertex pool outgrow their
//!   buffers). Each added character must be drawn, in its own pose.
//! - The camera's exposure, then a resize that changes the bloom chain's length.
//!
//! Multisampled without TAA or GTAO, so a frame depends only on the scene. Skips when there is no
//! GPU.

use glam::Vec3;
use pocket_assets::frame::{
    AnimView, EnvironmentView, InstanceUpdate, LightKindView, LightView, Look, Pose, RenderFrame,
};
use pocket_render::{Antialiasing, BackendChoice, CameraState, Gpu, Gtao, Renderer, demo};

const W: u32 = 480;
const H: u32 = 270;
const HERO: &str = "models/hero.glb";

fn gpu() -> Option<Gpu> {
    let gpu = Gpu::headless(BackendChoice::from_env()).ok();
    if gpu.is_none() {
        eprintln!("no GPU: skipped");
    }
    gpu
}

fn renderer(gpu: &Gpu, width: u32, height: u32) -> Renderer {
    let hero = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../samples/anim/models/hero.glb"
    ))
    .expect("samples/anim/models/hero.glb");
    let hero = pocket_assets::import::import_glb_bytes(&hero).expect("hero.glb imports");
    let mut r = Renderer::new(gpu, wgpu::TextureFormat::Rgba8UnormSrgb, width, height);
    r.set_antialiasing(Antialiasing::Msaa);
    r.set_gtao(Gtao::Off);
    r.add_model(HERO, &hero);
    r.add_model(demo::MIXED_MODEL, &demo::mixed_model());
    r
}

/// Character `i` (entity 100 + i) in a row facing the camera, its idle clip held at a time of its
/// own.
fn hero(i: u64) -> InstanceUpdate {
    InstanceUpdate {
        id: 100 + i,
        pose: Some(Pose {
            position: [-6.0 + 1.5 * i as f32, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
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
            time: 0.37 * i as f32,
            rate: 0.0,
            looped: true,
        }),
    }
}

/// A frame at `tick` carrying `instances`; with `reset` also the sun, the sky and a ground.
fn frame(tick: u64, reset: bool, instances: Vec<InstanceUpdate>, removed: Vec<u64>) -> RenderFrame {
    let mut f = RenderFrame {
        tick,
        t_s: tick as f64 / 60.0,
        dt_s: 1.0 / 60.0,
        reset,
        instances,
        removed,
        ..RenderFrame::default()
    };
    if reset {
        f.instances.push(InstanceUpdate {
            id: 1,
            pose: Some(Pose {
                position: [0.0; 3],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [30.0, 1.0, 30.0],
            }),
            look: Some(Look {
                mesh: "plane".into(),
                material: String::new(),
                color: [0.55, 0.55, 0.52, 1.0],
                metallic: 0.0,
                roughness: 0.8,
                transmission: None,
                ior: None,
                emissive: [0.0; 3],
                cast_shadows: false,
                visible: true,
            }),
            anim: None,
        });
        f.lights = Some(vec![LightView {
            id: 0,
            kind: LightKindView::Directional,
            position: [0.0; 3],
            direction: Vec3::new(-0.4, -1.0, -0.5).normalize().to_array(),
            color: [1.0, 0.96, 0.9],
            intensity: 6.0,
            range: 0.0,
            inner_deg: 0.0,
            outer_deg: 0.0,
            shadows: true,
        }]);
        f.cameras = Some(vec![]);
        f.environment = Some(EnvironmentView {
            sky: 0,
            sky_color: [0.3, 0.5, 0.8],
            ambient: 1.0,
            baked_gi: String::new(),
            neural_gi: String::new(),
            gi_intensity: 1.0,
            fog_density: 0.0,
            fog_color: [0.6, 0.7, 0.8],
            exposure_ev: 0.0,
            bloom: 0.3,
        });
    }
    f
}

fn camera() -> CameraState {
    CameraState::look_at(Vec3::new(0.0, 1.6, 8.0), Vec3::new(0.0, 1.0, 0.0))
}

/// The captured frame and the entities the id pass saw, with their coverage.
fn shoot(r: &mut Renderer, t: f64) -> (Vec<u8>, Vec<(u64, f32)>) {
    r.request_visible();
    let mut visible = None;
    for _ in 0..30 {
        let _ = r.capture_rgba(t);
        if let Some(v) = r.take_visible() {
            visible = Some(v);
            break;
        }
    }
    let rgba = r.capture_rgba(t).2;
    (rgba, visible.expect("the id pass answers"))
}

/// The share of pixels off by more than 8 levels in a channel.
fn differing(a: &[u8], b: &[u8]) -> f64 {
    assert_eq!(a.len(), b.len());
    let n = a
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.as_chunks::<4>().0)
        .filter(|(p, q)| (0..3).any(|i| p[i].abs_diff(q[i]) > 8))
        .count();
    n as f64 / (a.len() / 4) as f64
}

/// `kept`'s frame equals a fresh renderer's for the same scene, and every character in `heroes` is
/// drawn.
fn same_as_fresh(gpu: &Gpu, kept: &mut Renderer, heroes: &[u64], t: f64, what: &str) {
    let (a, seen) = shoot(kept, t);
    let mut fresh = renderer(gpu, W, H);
    fresh.set_camera_override(Some(camera()));
    fresh.apply(
        frame(1, true, heroes.iter().map(|&i| hero(i)).collect(), vec![]),
        t,
    );
    let (b, fresh_seen) = shoot(&mut fresh, t);
    for &i in heroes {
        let share = |v: &[(u64, f32)]| v.iter().find(|x| x.0 == 100 + i).map_or(0.0, |x| x.1);
        eprintln!(
            "{what}: character {i} covers {:.4} of the view (fresh renderer {:.4})",
            share(&seen),
            share(&fresh_seen)
        );
        assert!(
            share(&fresh_seen) > 0.001,
            "{what}: character {i} is in view"
        );
        assert!(
            (share(&seen) - share(&fresh_seen)).abs() < 0.0005,
            "{what}: character {i} covers {} of the view, a fresh renderer's {}",
            share(&seen),
            share(&fresh_seen)
        );
    }
    let d = differing(&a, &b);
    eprintln!("{what}: {:.4}% of the pixels differ", d * 100.0);
    assert!(d < 0.001, "{what}: {:.3}% of the pixels differ", d * 100.0);
}

#[test]
fn skinned_characters_added_later_draw_in_their_own_poses() {
    let Some(gpu) = gpu() else { return };
    let t = 1.0;
    let mut r = renderer(&gpu, W, H);
    r.set_camera_override(Some(camera()));
    r.apply(frame(1, true, vec![hero(0), hero(1)], vec![]), t);
    let _ = shoot(&mut r, t);
    // One replaced by another: as many jobs as before, different ones.
    r.apply(frame(2, false, vec![hero(2)], vec![100]), t);
    same_as_fresh(&gpu, &mut r, &[1, 2], t, "one replaced");
    // Six more: every buffer of the skinning pass grows.
    r.apply(frame(3, false, (3..9).map(hero).collect(), vec![]), t);
    same_as_fresh(&gpu, &mut r, &[1, 2, 3, 4, 5, 6, 7, 8], t, "six added");
}

#[test]
fn exposure_and_resizes_reach_the_display_transform() {
    let Some(gpu) = gpu() else { return };
    let t = 1.0;
    let scene = || {
        let mut f = demo::mixed(6);
        if let Some(e) = &mut f.environment {
            e.bloom = 0.3;
        }
        f
    };
    let camera = |exposure_ev: f32| CameraState {
        exposure_ev,
        ..demo::mixed_camera(6)
    };
    let fresh = |width: u32, height: u32, exposure_ev: f32| {
        let mut f = renderer(&gpu, width, height);
        f.set_camera_override(Some(camera(exposure_ev)));
        f.apply(scene(), t);
        f.capture_rgba(t).2
    };
    let mut r = renderer(&gpu, W, H);
    r.set_camera_override(Some(camera(0.0)));
    r.apply(scene(), t);
    let _ = r.capture_rgba(t);
    r.set_camera_override(Some(camera(1.0)));
    let d = differing(&r.capture_rgba(t).2, &fresh(W, H, 1.0));
    eprintln!("exposure: {:.4}% of the pixels differ", d * 100.0);
    assert!(
        d < 0.001,
        "exposure: {:.3}% of the pixels differ",
        d * 100.0
    );
    // A size whose bloom chain is a level shorter (five half-size levels instead of six).
    r.resize(160, 90);
    let d = differing(&r.capture_rgba(t).2, &fresh(160, 90, 1.0));
    eprintln!("resize: {:.4}% of the pixels differ", d * 100.0);
    assert!(d < 0.001, "resize: {:.3}% of the pixels differ", d * 100.0);
}
