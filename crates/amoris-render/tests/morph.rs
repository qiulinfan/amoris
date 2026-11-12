//! Morph targets on the GPU (docs/spec/animation.md 5a), with `tests/fixtures/scenes/morph`'s model
//! (its `gen_morph.py` writes it): a fully weighted target draws the mesh where the same mesh moved
//! by the target's offset draws it, for the unskinned plate (weights from a simulated pose and from
//! a clip, one target and two) and for the skinned body, whose offset turns with its joint (morph,
//! then skin); a target that moves one vertex raises the plate's outline there by its offset. On
//! the default device and on WebGPU's baseline. Skips when there is no GPU.

use amoris_assets::frame::{AnimView, InstanceUpdate, Look, Pose, RenderFrame};
use amoris_assets::mesh::ModelAsset;
use amoris_render::gpu::Minimal;
use amoris_render::{
    Antialiasing, BackendChoice, CameraState, Gpu, Gtao, OcclusionMode, Renderer, demo,
};
use glam::{Quat, Vec3};

const W: u32 = 640;
const H: u32 = 360;
const MODEL: &str = "models/morph.gltf";

fn model() -> ModelAsset {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/scenes/morph/models/morph.gltf"
    );
    amoris_assets::import::import_gltf(std::path::Path::new(path)).expect("the morph fixture")
}

/// A pose of the fixture's four nodes at rest, the bone turned by `turn`, with `weights`.
fn posed(asset: &ModelAsset, turn: Quat, weights: [f32; 3]) -> AnimView {
    let mut pose = Vec::new();
    for n in &asset.skeleton {
        let r = if n.name == "bone" {
            (Quat::from_array(n.rotation) * turn).to_array()
        } else {
            n.rotation
        };
        pose.extend_from_slice(&n.translation);
        pose.extend_from_slice(&r);
        pose.extend_from_slice(&n.scale);
    }
    AnimView {
        pose,
        tick_s: 1.0 / 60.0,
        morph: weights.to_vec(),
        ..AnimView::default()
    }
}

/// The fixture's node `part` (`plate` or `body`) at `at`, drawn by `anim`.
fn figure(part: &str, at: Vec3, anim: AnimView) -> InstanceUpdate {
    InstanceUpdate {
        id: 1,
        pose: Some(Pose {
            position: at.to_array(),
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        }),
        look: Some(Look {
            mesh: format!("{MODEL}#{part}"),
            material: String::new(),
            color: [1.0; 4],
            metallic: 0.0,
            roughness: 0.8,
            transmission: None,
            ior: None,
            emissive: [0.0; 3],
            cast_shadows: false,
            visible: true,
        }),
        anim: Some(anim),
    }
}

/// The image of `instances` (none: the background) seen from `eye` toward `target`.
fn shoot(
    gpu: &Gpu,
    asset: &ModelAsset,
    eye: Vec3,
    target: Vec3,
    instances: Vec<InstanceUpdate>,
) -> Vec<u8> {
    let mut r = Renderer::new(gpu, wgpu::TextureFormat::Rgba8UnormSrgb, W, H);
    r.set_antialiasing(Antialiasing::Off);
    r.set_gtao(Gtao::Off);
    r.set_occlusion(OcclusionMode::Off);
    r.add_model(MODEL, asset);
    r.apply(
        {
            // The demo scene's sun, sky and empty camera list, without its things.
            let lit = demo::mixed(1);
            RenderFrame {
                reset: true,
                dt_s: 1.0 / 60.0,
                instances,
                lights: lit.lights,
                cameras: lit.cameras,
                environment: lit.environment,
                ..RenderFrame::default()
            }
        },
        0.0,
    );
    r.set_camera_override(Some(CameraState::look_at(eye, target)));
    for _ in 0..3 {
        let _ = r.capture_rgba(0.0);
    }
    r.capture_rgba(0.0).2
}

/// The share of pixels that differ by more than a small amount.
fn differing(a: &[u8], b: &[u8]) -> f64 {
    let n = a
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.as_chunks::<4>().0)
        .filter(|(p, q)| (0..3).any(|i| p[i].abs_diff(q[i]) > 8))
        .count();
    n as f64 / f64::from(W * H)
}

/// The pixels where `shot` is not the background: per column, the topmost such row.
fn tops(shot: &[u8], background: &[u8]) -> Vec<Option<u32>> {
    let lit = |x: u32, y: u32| {
        let i = ((y * W + x) * 4) as usize;
        (0..3).any(|c| shot[i + c].abs_diff(background[i + c]) > 24)
    };
    (0..W).map(|x| (0..H).find(|&y| lit(x, y))).collect()
}

fn check(gpu: &Gpu, label: &str) {
    // A failed pipeline voids the frame without a word otherwise.
    amoris_render::stderr_log::init();
    let asset = model();
    let rest = Quat::IDENTITY;
    let quarter = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
    // The plate, seen face on.
    let (eye, target) = (Vec3::new(0.3, 0.1, 3.0), Vec3::new(0.3, 0.1, 0.0));
    let plate =
        |at: Vec3, anim: AnimView| shoot(gpu, &asset, eye, target, vec![figure("plate", at, anim)]);
    let right = Vec3::new(0.6, 0.0, 0.0);
    let still = plate(Vec3::ZERO, posed(&asset, rest, [0.0; 3]));
    let moved = plate(right, posed(&asset, rest, [0.0; 3]));
    let morphed = plate(Vec3::ZERO, posed(&asset, rest, [1.0, 0.0, 0.0]));
    let apart = differing(&still, &moved);
    let same = differing(&morphed, &moved);
    eprintln!("{label}: plate: moved {apart:.4} of the pixels; morphed against moved {same:.5}");
    assert!(
        apart > 0.01,
        "{label}: the reference did not move ({apart})"
    );
    assert!(
        same < 0.002,
        "{label}: the fully weighted target is not the moved plate ({same})"
    );
    // The same weight from a clip (the cubic "ease" at its end).
    let clip = plate(
        Vec3::ZERO,
        AnimView {
            clip: "ease".into(),
            time: 1.0,
            rate: 0.0,
            looped: false,
            ..AnimView::default()
        },
    );
    let same = differing(&clip, &moved);
    eprintln!("{label}: plate by its clip against moved {same:.5}");
    assert!(
        same < 0.002,
        "{label}: the clip's weight is not the moved plate ({same})"
    );
    // Two targets: "lift" raises the centre vertex 0.3 m above the plate's centre, 0.1 m above its
    // top edge; with "right" too, the lifted plate moved.
    let lifted = plate(Vec3::ZERO, posed(&asset, rest, [0.0, 1.0, 0.0]));
    let lifted_moved = plate(right, posed(&asset, rest, [0.0, 1.0, 0.0]));
    let both = plate(Vec3::ZERO, posed(&asset, rest, [1.0, 1.0, 0.0]));
    let same = differing(&both, &lifted_moved);
    eprintln!("{label}: plate with both targets against the lifted plate moved {same:.5}");
    assert!(same < 0.002, "{label}: two targets ({same})");
    let background = shoot(gpu, &asset, eye, target, Vec::new());
    let (flat, peak) = (tops(&still, &background), tops(&lifted, &background));
    let columns: Vec<u32> = (0..W).filter(|&x| flat[x as usize].is_some()).collect();
    assert!(columns.len() > 50, "{label}: the plate is not in view");
    // Metres to pixels from the plate's width (1 m).
    let per_m = (columns[columns.len() - 1] - columns[0] + 1) as f32;
    let centre = columns[columns.len() / 2] as usize;
    let rise = flat[centre].unwrap() as f32 - peak[centre].unwrap() as f32;
    eprintln!(
        "{label}: the lifted centre rises {rise} px, 0.1 m is {:.1} px",
        0.1 * per_m
    );
    assert!((rise - 0.1 * per_m).abs() <= 2.5, "{label}: rise {rise} px");
    // The corners do not move (the edge toward the lifted centre rises a fifth of a pixel a pixel).
    let edge = columns[0] as usize;
    let moved_by = flat[edge].unwrap().abs_diff(peak[edge].unwrap());
    assert!(moved_by <= 1, "{label}: a corner moved {moved_by} px");

    // The skinned body turned a quarter about z: its offset along +x turns to +y.
    let (eye, target) = (Vec3::new(1.5, 1.4, 3.5), Vec3::new(1.5, 1.4, 0.0));
    let body =
        |at: Vec3, anim: AnimView| shoot(gpu, &asset, eye, target, vec![figure("body", at, anim)]);
    let up = Vec3::new(0.0, 0.6, 0.0);
    let turned = body(Vec3::ZERO, posed(&asset, quarter, [0.0; 3]));
    let moved = body(up, posed(&asset, quarter, [0.0; 3]));
    let morphed = body(Vec3::ZERO, posed(&asset, quarter, [0.0, 0.0, 1.0]));
    let wrong = body(right, posed(&asset, quarter, [0.0; 3]));
    let apart = differing(&turned, &moved);
    let same = differing(&morphed, &moved);
    let skin_first = differing(&morphed, &wrong);
    eprintln!(
        "{label}: body: moved {apart:.4}; morphed against moved {same:.5}, against skinned then \
         morphed {skin_first:.4}"
    );
    assert!(
        apart > 0.01,
        "{label}: the body's reference did not move ({apart})"
    );
    assert!(same < 0.002, "{label}: morph, then skin ({same})");
    assert!(
        skin_first > 0.01,
        "{label}: the offset did not turn ({skin_first})"
    );
}

#[test]
fn a_fully_weighted_target_moves_the_vertices_by_its_offsets() {
    let choice = BackendChoice::from_env();
    let Ok(gpu) = Gpu::headless(choice) else {
        eprintln!("no GPU: skipped");
        return;
    };
    check(&gpu, "default device");
    drop(gpu);
    let baseline = Gpu::headless_with(choice, Minimal::parse("features,limits,timestamps"))
        .expect("the baseline device");
    check(&baseline, "WebGPU baseline");
}
